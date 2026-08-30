# SMA-605 — a cargo invocation through a variable, seen by A8 and A10

Status: design, revision 2 (after adversarial review), 2026-08-30.
Supersedes limitation **L10** of
`docs/superpowers/specs/2026-08-29-sma-599-cargo-invocation-invariants-design.md`, and closes
**L2**'s `source` half with a transitive, cycle-guarded closure.

## 1. Problem

`ci/affected-graph/cargo_moon_parity.py` finds cargo through `CARGO_INVOCATION_RE`, which needs
the literal word `cargo` followed by a lock-resolving verb. Two assertions read that regex:

* **A8** (`check_cargo_locked`, `check_cargo_locked_scripts`, `check_dockerfile_locked`) asserts
  `--locked`. An unlocked cargo call re-resolves the graph and rewrites an inconsistent
  `rs/Cargo.lock` in place.
* **A10** (`check_cargo_config_inputs`) asserts the `rs/.cargo/config.toml` task input. Its
  derivation filters on the same regex.

An invocation that reaches cargo through a variable carries no literal `cargo` token. Neither
assertion sees it.

## 2. Method

Revision 1 of this spec measured by grep over physical file text. That was wrong, and the
adversarial review caught it. A8 and A10 do not read file text: they read moon's **resolved
blob** (`command` + `script` + `args`, never the YAML) and, for a script, **logical lines** with
comment, heredoc and operator-span regions already removed, split on `[;&|]+`.

Every number in §3 is now produced by importing `cargo_moon_parity.py`, calling
`moon_projects()`, `derive_cargo_tasks()`, `script_cargo_lines()` and — for anything measured
AFTER the change — `task_script_closure()` rather than `task_script_refs()`, which returns only
DIRECT references and cannot reach a sourced ecosystem module, and
running the candidate regexes through the same `_line_regions` / `_join` /
`COMMAND_SPLIT_RE` pipeline the production scanner uses. Where a number concerns the corpus at
large rather than the reachable corpus, it says so.

## 3. Measurements

### M1 — no variable holding cargo is used in command position

The repo holds **zero** instances of `"$VAR" <verb>` where the variable holds a cargo path.
Instances of the *shape* do exist — `"$RELEASE_PLZ_BIN" update`, `git -C "$dir" add -A` — and M4
lists them. Revision 1 said "zero instances of the shape", which M4 contradicted two paragraphs
later.

SMA-605 names `ci/release-parity/ecosystems/release-plz.sh:63` as the one call site. That line
assigns `CARGO_BIN`, but no line runs `"$CARGO_BIN" <verb>`. Line 152 uses the value as an
environment variable instead:

```sh
if ! out="$(cd "$1" && CARGO="$CARGO_BIN" CARGO_NET_OFFLINE=true "$RELEASE_PLZ_BIN" update …)"
```

### M2 — a fourth indirection shape, not named in the issue

`CARGO=<path> <tool>` reaches cargo through a tool that shells out to it. This is what the repo
actually does. None of the three options in the issue covers it.

### M3 — only three scripts are followed today

`task_script_refs` reads a task's own resolved blob. Across the whole derived set that yields
**three** scripts: `ci/actionlint/run.sh`, `ci/publish-metadata/run.sh`,
`ci/version-lockstep/run.sh`. Every claim about a "followed" corpus below is against those three.

### M4 — naive widening costs ONE waiver, not three

Accepting any variable in command position before a lock-resolving verb, measured through the
production pipeline:

| Corpus | rows |
|---|---|
| the three **followed** scripts | **1** |
| all `ci/**/*.sh` | 3 |

The one reachable row is `ci/publish-metadata/run.sh:1647`,
`echo "negative control: $failures check(s) failed to bite"` — prose in an `echo`, which the
conservative rule does not strip (SMA-599 L8).

The two unreachable rows are `ci/release-parity/ecosystems/release-plz.sh:152`
(`"$RELEASE_PLZ_BIN" update`) and `ci/release-plan/run.sh:149` (`git -C "$dir" add -A`).
Revision 1 claimed the second "pulls a cargo-free task into both A8 and A10 scope". That was
wrong twice: `moon.yml` names `ci/release-plan/**/*` only as an **input glob** (`:220`) and in a
comment (`:215-216`), never as an invocation, so no task follows it; and `add` is absent from
`CONFIG_SENSITIVE_VERBS`, so A10 could not have taken it either. This is the rows-vs-reachable
conflation SMA-599 §2.2 already warns about.

### M5 — a cargo-named command-position arm measures at zero rows

If the variable's own name must contain `cargo`, case-insensitive: **0 rows** on the followed
corpus and **0** across all `ci/**/*.sh`.

### M6 — the environment arm must key on the exact name `CARGO`

Line 152 carries two cargo-named prefixes: `CARGO="$CARGO_BIN"` and `CARGO_NET_OFFLINE=true`.
Only the first names the cargo binary. `CARGO_NET_OFFLINE`, `CARGO_HOME` and `CARGO_TERM_COLOR`
configure cargo; they do not redirect it. A "name mentions cargo" predicate reports them.

Both predicates measure identically on today's corpus, so this is decided on principle.

Revision 1 justified the arm's lookahead with a consecutive-prefix failure. That justification
was wrong: it was measured against the **rejected** "mentions cargo" predicate. Under exact
`CARGO=`, `CARGO_NET_OFFLINE=true` never matches, so nothing is consumed and the two variants
behave alike. §6.2 states the lookahead's real job instead.

### M7 — no substitution-body scan is needed in A8

`_classify_shell_line` splits a logical line on `[;&|]+` **before** matching, and that split
already reaches inside a `$( … )` body. Measured on line 152:

```
seg[1] = ' CARGO="$CARGO_BIN" CARGO_NET_OFFLINE=true "$RELEASE_PLZ_BIN" update 2>'
arm-2 match: ' CARGO="$CARGO_BIN"'
```

An earlier draft added a substitution-body scan to A8. It generalised from a probe that stripped
substitutions, which the production scanner does not do. `_cwd_inside_rs` already scans both the
stripped text and each substitution body, so A10 needs nothing either. The mechanism is dropped.

### M8 — the resolved blob excludes `env`

`moon_projects()` joins `command` + `script` + `args` only. A Moon task declaring
`env: {CARGO: /p}` beside a wrapper is invisible to the blob scan **whatever** arm 2 does. No
task declares `CARGO` today, but that is a fact about the corpus, not about coverage. Recorded
as R8.

### M9 — `SCRIPT_REF_RE` cannot match a shellcheck `source=` directive

The regex requires `^` or `[\s;&|(]` before the path. Measured:

```
'# shellcheck source=ci/release-parity/ecosystems/release-plz.sh'  ->  []
'source "$HERE/ecosystems/$ECOSYSTEM.sh"'                          ->  []
'bash ci/release-parity/run.sh --ecosystem release-plz'            ->  ['ci/release-parity/run.sh']
```

### M10 — naive one-level-deeper following follows PROSE

Running `SCRIPT_REF_RE` over a followed script's own text yields six new scripts, and **every
edge is a comment or a string constant**:

| Edge | Origin |
|---|---|
| `publish-metadata → osv`, `→ next-env` | comment, `:9-10` |
| `publish-metadata → release-parity` | comments `:1686`, `:1726` |
| `actionlint → affected-graph` | comments `:2016`, `:2041`, `:2046` |
| `actionlint → cargo-lock-integrity` | the `T_CARGO_LOCK_STEP_REQUIRED` pin array, `:2152-2154` |
| `actionlint → release-plan` | the same shape |

Cost: six scripts into A8 scope on the strength of comments, one new waiver
(`ci/cargo-lock-integrity/run.sh:60`, an `::error::` string), and **zero** true positives. It
also still does not reach `release-plz.sh`. Rejected in §4.

### M11 — `repo:release-parity` is not derived at all

`derive_cargo_tasks` reports `{}` for every `release-parity*` task, because
`ci/release-parity/run.sh` holds **0** cargo lines — the cargo lives one level down in the
sourced module. So `task_script_refs` never runs for it, and no amount of source resolution helps
unless the derivation also follows before deciding `kind`. Closing L2 is two coupled changes,
not one.

### M12 — the source corpus is one statement

`ci/**/*.sh` holds exactly **one** `source`/`.` statement:

```sh
# ci/release-parity/run.sh
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"   # :7
source "$HERE/ecosystems/$ECOSYSTEM.sh"                # :21
```

`ci/release-parity/ecosystems/` holds three modules. Under today's classifier they produce **0**
reporting rows between them. With arm 2 shipped, `release-plz.sh:152` becomes **one** reporting
row — the change's single live true positive, and its single new waiver.

## 4. Decision

Ship **three** things:

1. **Arm 1** — a cargo-named variable in command position.
2. **Arm 2** — the `CARGO=` environment prefix, with wrapper semantics.
3. **An execution-only source resolver** — a transitive, cycle-guarded closure over `source` /
   `.` statements — closing SMA-599 L2's `source` half, plus the derivation reorder M11
   requires.

Without (3), (1) and (2) measure at zero rows on the reachable corpus **by construction**, and
the §6.3 differential cannot fail — cover that no corpus can validate and that will rot. With
(3), arm 2 has a live true positive to prove itself against.

Rejected, with reasons:

* **Naive widening (issue option 1, unmodified).** Its numeric cost is one waiver (M4), not the
  three revision 1 claimed, so the numeric argument does not carry the decision. The argument
  that does: `add`, `run`, `test`, `check`, `build` and `update` are ordinary English and CLI
  words. In a repo whose gates are shell scripts full of prose diagnostics, the *future*
  false-positive rate of an unconstrained variable-plus-verb match is unbounded, and each one
  lands on a required check with a message no reviewer will immediately understand (SMA-599 L4).
* **Ban the indirection (issue option 2).** A name-based ban on `CARGO=`-style assignment hits
  three sites, two of which are unrelated pin arrays in `ci/actionlint/run.sh`. It constrains how
  a gate script may name a constant, and it still misses M2.
* **Accept and document (issue option 3).** Satisfies AC 2 vacuously.
* **Naive one-level-deeper following.** M10: six prose edges, one waiver, zero true positives,
  and it still misses the target.
* **Resolve the shellcheck `source=` directive instead of globbing.** Reaches `release-plz.sh`
  exactly and no other module, but coverage vanishes silently if the directive is deleted, and
  it leaves `semantic-release.sh` and `python-semantic-release.sh` unscanned. Both are real code
  a Moon task executes.

## 5. Design

### 5.1 The merged match list

`CARGO_INVOCATION_RE` is unchanged. Two new regexes sit beside it. A helper returns one
start-sorted list of `CargoMatch(start, end, verb, kind)` records, where `kind` is `literal`,
`var` or `env`. A namedtuple rather than a raw `re.Match`, for two reasons: `_classify_shell_line`
needs `start`/`end` for its per-invocation tail arithmetic, and the `--no-deps` carve-out needs
the **verb** (see 5.4).

Arm 1's name filter runs **before** the list is merged. A rejected match left in the list would
still act as a `stop` boundary and truncate the preceding invocation's tail.

### 5.2 Arm 1 — a cargo-named variable in command position

```python
CARGO_VAR_CMD_RE = re.compile(
    r"""(?:^|[\s;&|(])["']?\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?["']?[^\S\n]+"""
    r"(?:\+\S+[^\S\n]+)?(" + "|".join(LOCK_RESOLVING_VERBS) + r")\b"
)
```

The captured name is post-filtered on `"cargo" in name.lower()`. The predicate is a function, so
the rejected name can appear in a diagnostic.

This is a real invocation with a tail, so a `--locked` in its own tail satisfies A8 — with one
stated exception, the `--no-deps` carve-out in 5.4.

Arm 1 reports **zero** rows on the corpus even after the resolver lands (M5, M12): only arm 2
gains a live true positive. Its constant therefore carries the same warning `FFI_MARKERS` already
carries for `maturin` — that this is forward cover, and must not be mistaken for measured
coverage.

### 5.3 Arm 2 — the `CARGO=` environment prefix

```python
_ENV_ASSIGN = r"""[A-Za-z_][A-Za-z0-9_]*=(?:"[^"]*"|'[^']*'|[^\s;&|]*)"""
CARGO_ENV_PREFIX_RE = re.compile(
    r"""(?:^|[\s;&|(])CARGO=(?:"[^"]*"|'[^']*'|[^\s;&|]*)"""
    r"(?=(?:[^\S\n]+" + _ENV_ASSIGN + r")*[^\S\n]+(?!" + _ENV_ASSIGN + r")\S)"
)
```

The name is exactly `CARGO` (M6). There is no verb requirement: the tool's verbs belong to the
tool. The trailing word is read through a **lookahead** so that `export CARGO=/p` as the last
token on a line — an assignment with nothing to run — produces no row. That is the lookahead's
job; M6 records why revision 1's justification for it was wrong.

The lookahead skips a run of further `NAME=value` assignments and then demands a
**non-assignment** command token, so `CARGO=/p CARGO_HOME=/x` — which runs nothing — produces no
row, while `CARGO=/p CARGO_HOME=/x tool run` still does (M36). Arm 1's separators are bounded the
same way arm 2's lookahead is.

The lookahead's character class is **horizontal whitespace only** (`[^\S\n]`), never `\s`. `\s`
crosses a physical line, and fifteen real moon blobs are multi-line `script:` blocks, so a
`\s`-based lookahead makes `export CARGO=/p` match an unrelated command on the NEXT line — a
live false positive on the blob arm, pinned by M5. The LEADING separator keeps `\s` deliberately:
a newline before `CARGO=` really does start a command.

Arm 2 carries **wrapper** semantics, the rule the three FFI tasks already carry. You cannot pass
`--locked` through `CARGO=<path> <tool>`, so a match always needs an `ALLOW_UNLOCKED_CARGO` or
`ALLOW_UNLOCKED_CARGO_SCRIPT` entry. A flag never satisfies it.

### 5.4 Integration — every producer and consumer of `kind`

* **`_classify_shell_line`** builds rows from the merged list. `ScriptCargoLine` gains a `kind`
  field.
* **The report predicate is factored into one helper**, `_row_reports(line)`, defined as
  `line.kind == "env" or (line.resolves and not line.locked)`. It is used by **both**
  `check_cargo_locked_scripts`' emission loop **and** its waiver-health loop. Without this, an
  `env` row whose tool carries `--locked` is emitted (kind-aware) but its waiver then reads as
  stale (`hits == []`), so the row is permanently red and unwaivable. The adversarial review
  found this against this spec's own fixture. It is `_row_reports`, not `_reports`: `self_test`
  already has a LOCAL helper called `_reports`, and a nested `def` makes that name local for the
  whole enclosing function, so a module-level `_reports` is unreachable from every fixture
  (UnboundLocalError, measured during implementation).
* **The `--no-deps` carve-out keys on the matched verb**, not on `CARGO_METADATA_RE` over
  `group(0)`. `CARGO_METADATA_RE` needs the literal lowercase `cargo`, so for an arm-1 match
  `"$CARGO_BIN" metadata --no-deps` it never fires and the call reports — contradicting
  SMA-599 D4, which argues that demanding `--locked` on a non-resolving call is cargo-cult
  compliance.
* **`derive_cargo_tasks`**: arm 2 folds into `is_wrapper` (it *is* wrapper semantics, and reuses
  the existing allowlist contract); arm 1 joins the `literal` branch. Precedence stays
  wrapper > literal > script.
* **`check_cargo_locked`'s blob arm**: same split. Arm 1 is satisfied by `--locked` in the blob;
  arm 2 needs a waiver.
* **`check_dockerfile_locked`** emits rows from the merged list, but its floor counts **literal**
  matches only. Counting merged matches would let an `ENV CARGO=/usr/local/bin/cargo …` line
  satisfy `seen > 0` after the real `RUN cargo build --locked` line was deleted — the
  floor-satisfied-by-a-non-invocation vacuity mode the file guards against everywhere else.

### 5.5 The source resolver (SMA-599 L2, transitive)

A new `script_source_refs(path, root)` returns the scripts a script **executes** through a
`source`/`.` statement. `root` is not optional: it bounds resolution to files inside the
repository, so a `source /etc/profile` resolves to nothing rather than pulling unreviewed text
into A8's corpus (M27). Bare `ci/**/*.sh` mentions in script text are **not** followed: M10
measures that as six prose edges and zero true positives.

Resolution rules, sized to M12's single statement:

* `$HERE` / `${HERE}` and the `$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)` idiom resolve to
  the script's own directory.
* A remaining unresolved `$VAR` segment becomes a glob. `"$HERE/ecosystems/$ECOSYSTEM.sh"`
  therefore yields all three ecosystem modules, not only the default one.
* A `source` whose target resolves to nothing raises `MoonOutputError`, the
  infrastructure-never-a-silent-pass contract `task_script_refs` already uses.
* The scan reads **executable text**, not raw text (`_executable_text`), reusing
  `script_cargo_lines`' own `_line_regions` and heredoc walk. Without this, "execution only" was
  false in a way that ABORTED the gate rather than merely over-reporting: a `source ./missing.sh`
  inside a heredoc body matched, and the resolver raised on the absent target. Measured on the
  CodeRabbit PR review, fixed with a fixture and a mutation.

`task_script_closure(projects, root, target)` returns `task_script_refs` plus the full
`script_source_refs` closure. It is a breadth-first **transitive** walk with no depth limit,
guarded by a visited set keyed on the resolved path, so a mutual `source` terminates. Depth is a
property of the CORPUS, not of the algorithm: today the corpus is depth 2 (one task blob, one
`source` statement). Earlier drafts of this spec said "one level", which described the corpus and
not the code. Every present caller of
`task_script_refs` moves to it: `derive_cargo_tasks`' `script` branch (which is what M11's
reorder needs), `check_cargo_locked_scripts`, and `check_cargo_config_inputs`.

A floor, `REQUIRED_SOURCED_SCRIPTS`, asserts that `ci/release-parity/run.sh` still resolves to
the three ecosystem modules. Without it a rename empties the closure silently, which is the
SMA-553 failure class.

### 5.6 A10 symmetry

A10 does not use `_classify_shell_line`; it runs `CONFIG_SENSITIVE_RE` over raw blob-plus-file
text. It therefore needs its **own** arm-1 regex, `CARGO_VAR_CMD_SENSITIVE_RE`, built from
`CONFIG_SENSITIVE_VERBS` and not from `LOCK_RESOLVING_VERBS`. Reusing arm 1 would pull
`"$CARGO_BIN" tree`, `deny` and `update` into A10 scope and nothing would red — the accident
SMA-599 D9 spent a round removing.

Arm 2 makes a task sensitive unconditionally: the tool's inner cargo may compile, and A10 cannot
know. The cwd rule is unchanged and still applies to both arms.

## 6. Verification

### 6.1 Self-test fixtures

Positive fixtures prove the arms fire. Negative fixtures pin the narrowing, so a later
"simplification" of either predicate reds the self-test rather than the corpus. **Script** and
**blob** fixtures are listed separately, because revision 1 had only script fixtures and the
review showed that three of §5.4's integration points were therefore unasserted.

Script fixtures, through `script_cargo_lines`:

| Fixture | Expected |
|---|---|
| `"$CARGO_BIN" build` | A8 row |
| `"$CARGO_BIN" build --locked` | no row |
| `"$CARGO_BIN" metadata --no-deps` | no row (the D4 carve-out) |
| `"$CARGO_BIN" tree` | A8 row, **no** A10 row |
| `CARGO=/p release-plz update` | A8 row, wrapper rule |
| `CARGO=/p release-plz update --locked` **plus a waiver** | **no rows at all** |
| the same inside `$( … )` | A8 row (M7 pin) |
| `export CARGO=/p` as the last token | no row (the lookahead) |
| `CARGO_NET_OFFLINE=true tool update` | no row |
| `git -C "$dir" add -A` | no row |
| `echo "… $failures check(s) …"` | no row |
| `"$RELEASE_PLZ_BIN" update` | no row |

Blob fixtures, through a `projects` dict with **no** script reference:

| Fixture | Expected |
|---|---|
| blob `"$CARGO_BIN" build` | `derive_cargo_tasks` → `literal`; A8 row |
| blob `CARGO=/p release-plz update` | `derive_cargo_tasks` → `wrapper`; A8 row |
| the same, cwd inside `rs/`, no `rs/.cargo/config.toml` | A10 row |
| `rs/Dockerfile` holding only `ENV CARGO=/p CARGO_HOME=/q` | A8 **floor** row |

Resolver fixtures:

| Fixture | Expected |
|---|---|
| a script sourcing `"$HERE/sub/$X.sh"` with two matching files | both in the closure |
| a source cycle | terminates, no repeat |
| a source resolving to nothing | `MoonOutputError` |

`self_test`'s closing line stays "all ten assertions", and `EXPECTED_FINDING_KEYS` is unchanged:
this widens A8 and A10 and adds no assertion.

### 6.2 Mutation proofs (AC 2)

**MEASURED against the final code: 42 mutations, 42 killed, 0 survivors.** Each was applied to
`cargo_moon_parity.py`, `--self-test` was run, and the file restored. Every mutation label below
is the edit that was actually performed, not a paraphrase of it. M29-M42 were added by the
CodeRabbit PR review; re-running the whole battery after those fixes is what caught M25's fixture
going stale (see below).

| # | Mutation | Result |
|---|---|---|
| M1 | delete arm 1 from the merged list | KILLED |
| M2 | delete arm 2 from the merged list | KILLED |
| M3 | relax arm 1's name filter to any name | KILLED |
| M4 | arm 2's lookahead -> a **consuming** tail (`(?=[^\S\n]+\S)` -> `[^\S\n]+\S`) | KILLED |
| M5 | let arm 2's lookahead cross a **newline** (`[^\S\n]` -> `\s`) | KILLED |
| M6 | disable end-offset de-duplication | KILLED |
| M7 | sort the merged list by position before kind | KILLED |
| M8 | make `_row_reports` kind-blind (both loops) | KILLED |
| M9 | kind-blind, **emission loop only** | KILLED |
| M10 | kind-blind, **waiver-health loop only** | KILLED |
| M11 | revert the `--no-deps` carve-out to `CARGO_METADATA_RE` | KILLED |
| M12 | revert `_classify_shell_line` to the literal regex | KILLED |
| M13 | drop arm 2 from `derive_cargo_tasks`' blob branch | KILLED |
| M14 | drop arm 1 from `derive_cargo_tasks`' blob branch | KILLED |
| M15 | drop the arms from `check_cargo_locked`'s blob test | KILLED |
| M16 | drop arm 2 from `check_cargo_locked`'s wrapper test | KILLED |
| M17 | count merged matches in the Dockerfile floor | KILLED |
| M18 | revert the Dockerfile to the literal-only scan | KILLED |
| M19 | reuse A8's verb list for A10's arm | KILLED |
| M20 | drop A10's arm 1 | KILLED |
| M21 | drop arm 2's unconditional sensitivity | KILLED |
| M22 | drop the transitive source closure | KILLED |
| M23 | resolve a multiply-assigned variable instead of globbing | KILLED |
| M24 | drop the source cycle guard | KILLED (by HANG — see below) |
| M25 | follow bare `ci/**/*.sh` mentions too | KILLED |
| M26 | silently skip a source resolving to nothing | KILLED |
| M27 | drop the repo-containment guard on a resolved source | KILLED |
| M28 | drop the resolver's entry `path.resolve()` | KILLED |
| M29 | scan RAW text for `source` statements (drop `_executable_text`) | KILLED |
| M30 | blame `FFI_MARKERS` for every wrapper cause | KILLED |
| M31 | drop `_executable_text`'s heredoc-open-at-EOF guard | KILLED |
| M32 | return UNRESOLVED closure members from `task_script_closure` | KILLED |
| M33 | compare against an unresolved `root` in `check_cargo_locked_scripts` | KILLED |
| M34 | let arm 1 span a newline (`[^\S\n]+` -> `\s+`) | KILLED |
| M35 | let A10's arm 1 span a newline | KILLED |
| M36 | accept an env prefix with no command after it | KILLED |
| M37 | keep the redundant env row beside a literal cargo call | KILLED |
| M38 | truncate the emitted waiver key to 100 chars again | KILLED |
| M39 | drop the Dockerfile `ENV CARGO=` rule | KILLED |
| M40 | match `CARGO=` inside a quoted ENV **value** | KILLED |
| M41 | blob arm: raw env regex instead of merged matches | KILLED |
| M42 | `check_cargo_locked`: raw env regex instead of merged matches | KILLED |

**SEVEN first-pass survivors, and what each bought.** A survivor is evidence about the FIXTURES,
never a result to accept, so each one is recorded with the fixture that now kills it. None was
predicted by the design: six surfaced during implementation or local review, and M25 only in the
PR review round.

| First-pass survivor | Why it survived | Fixture added |
|---|---|---|
| M4 | §5.3's original justification for the lookahead was wrong. Under the exact-`CARGO` predicate, `export CARGO=/p` reports nothing whether the tail is consumed or looked ahead. Measured over eight candidate shapes, exactly ONE separates them. | `CARGO=/p CARGO=/q tool update` -> two `env` matches. Consuming eats the second prefix's leading separator, so `finditer` resumes mid-token and finds one. |
| M10 | the adversarial review predicted this exactly. The waiver-health loop can be left kind-blind alone, and then an honest waiver for an `env` row reads as STALE — permanently red, unwaivable. | the waiver round trip: every `env` row waived -> **no rows at all**. |
| M9 | that round-trip fixture only exercises the WAIVED path, so the emission half stayed unpinned. | the unwaived assertion: `CARGO=/p release-plz update --locked` must still report. |
| M21 | a blob-level `CARGO=` is already `wrapper` by derivation, so `kind == "wrapper"` covered it. The clause is load-bearing only one level down. | a `CARGO=` inside a FOLLOWED SCRIPT — SMA-599 L13's shape, where the task derives as `script`. |
| M27 | the repo-containment guard was added during the plan's own self-review and shipped with no fixture. | a `source` naming a file outside the repo must raise. |
| M28 | `resolve()` is a no-op at every current call site, so it was unasserted hardening. | `script_source_refs` on a RELATIVE path must resolve identically. |
| M25 | it KILLED until M29's fix landed, then went stale. The fixture put the bare `ci/**/*.sh` mention in a COMMENT, and `_executable_text` strips comments — so after the heredoc fix the mutation had nothing left to find. Only re-running the WHOLE battery after a fix, rather than the mutations that fix introduced, exposed it. | an EXECUTABLE bare mention (a pin-array string constant), which is the real corpus shape: every one of the six measured prose edges is a comment **or a constant**, and a constant is executable text. |

**Seven entries were live defects before they were mutations.** M5 (below), M29, M32 and
M34-M37 — the round-4 batch, which is the one that found the sharpest bug in the design:

**M37 made a correct line UNWAIVABLE.** `CARGO=/p cargo build` produced BOTH an env row and a
literal row, because their end offsets differ so the de-duplication did not fire. The two rows
carry the SAME segment text, so every `ALLOW_UNLOCKED_CARGO_SCRIPT` key for that line is
permanently AMBIGUOUS — SMA-599 L15, reached by a route L15 did not anticipate. Measured on
`CARGO=/p cargo build --locked`, a correctly locked call that reported twice and could not be
cleared. An env prefix whose COMMAND is cargo itself is not indirection, and is now dropped.

**M34/M35** are M5's defect in arm 1, on both A8's and A10's variants: `"$CARGO_BIN"` on one line
and `build` on the next read as one invocation. **M36**: `CARGO=/p CARGO_HOME=/x` sets two
variables and runs nothing, so it is not a wrapper — the lookahead now skips further assignments
and demands a real command, which keeps `CARGO=/p CARGO_HOME=/x tool run` matched. **M39** is the
counterpart: a Dockerfile `ENV CARGO=` carries no command yet redirects cargo for every later
`RUN`, so it takes its own rule rather than the shell-prefix one.

**M32** is the worst-behaved of the three: with a SYMLINKED `root`, `task_script_closure` mixed
path forms — `task_script_refs` builds `root / rel` and keeps the caller's form, while
`script_source_refs` resolves — so `check_cargo_locked_scripts`' `path.relative_to(root)` raised
`ValueError`. That is NOT in `INFRA_ERRORS`, so it escaped as a TRACEBACK rather than the rc-2
infrastructure classification the gate contracts for. macOS makes it reachable in ordinary use:
`/tmp` is a symlink to `/private/tmp`. Every closure member is now returned resolved, and
consumers compare against a resolved root.

**M29:** a `source`
inside a heredoc BODY matched, because `script_source_refs` scanned raw text while claiming to be
execution-only. That one did not merely over-report — it raised `MoonOutputError` and **aborted
the gate at rc 2** on a benign script. Found by the CodeRabbit PR review, fixed by reusing
`script_cargo_lines`' own heredoc walk (`_executable_text`) rather than by documenting it.

**M5 was not a mutation first — it was a live defect**, found by the CodeRabbit local review and
confirmed by measurement. `\s` in the trailing lookahead crosses a newline, so
`export CARGO=/p` followed by an unrelated command on the NEXT line matched as a wrapper. That
matters because **fifteen real moon blobs are multi-line `script:` blocks**, so it was a live
false positive on the blob arm, not a hypothetical one. It is in the accepted false-positive
direction — loud, not silent — but it would have fired on real input.

**M24 kills by HANG, not by a red.** Removing the cycle guard makes `task_script_closure` loop
forever on a mutual `source`; the harness reports TIMEOUT rather than rc 1. In CI that is a job
timeout, which is loud rather than silent, so the guard IS asserted — but by liveness, not by an
assertion message. Recorded rather than smoothed over.

### 6.3 Corpus differential (AC 3, AC 4)

Diff four measures before and after. The "before" run is captured:

| Measure | Before |
|---|---|
| `derive_cargo_tasks` | 63 tasks |
| A8 blob `matched` | 60 |
| A10 `in_scope` | 58 |
| findings a1–a10 | 0 rows |

MEASURED after, and every movement explained:

| Measure | Before | After | Why |
|---|---|---|---|
| `derive_cargo_tasks` | 63 | **66** | the three `release-parity*` tasks become derived, kind `script`. Before the resolver, `ci/release-parity/run.sh` held 0 cargo lines and the cargo lived one level down (M11), so no task followed it. |
| A8 blob `matched` | 60 | 60 | unchanged, and correctly so: none of the three carries a cargo verb or a `CARGO=` in its OWN blob. The blob arm is not what found them. |
| A10 `in_scope` | 58 | 58 | unchanged. This was the spec's one open question. The three new tasks are NOT in A10's scope, because `_cwd_inside_rs` is False for them: the ecosystem modules `cd` into a `mktemp -d` fixture, never into `rs/`. The cwd rule excluded them without a waiver, which is the outcome A10's design intends. |
| findings a1–a10 | 0 rows | 0 rows | one new reporting row appeared and one new waiver clears it — see below. |

The one row is `ci/release-parity/ecosystems/release-plz.sh:152`, the arm-2 shape this whole
change exists to see, and the only one in the repo. It takes one new
`ALLOW_UNLOCKED_CARGO_SCRIPT` entry. The waiver's reason is measured, not argued: the call runs
against a disposable fixture OUTSIDE the repo (`ci/release-parity/run.sh:43` makes it with
`mktemp -d`, `:48` passes it to `ecosystem::run_update`, which `cd`s into it), so it resolves but
cannot rewrite `rs/Cargo.lock`.

**AC 4, measured.** `ci/affected-graph/run.sh --negative-control` and `ci/affected-graph/run.sh`
both exit 0. No expected-set movement in any case, including `lockfile->all-lint` and
`kernel->consumer-tasks`.

## 7. Documentation

* SMA-599 spec **L10** — rewritten to record this decision and point here.
* SMA-599 spec **L2** — updated: source statements are now followed transitively; bare mentions are
  not, with M10's reason.
* SMA-599 spec **L11** — a pointer: L10 is closed for the variable shape, while L11's subcommand
  shape (`cargo llvm-cov`, `insta`, `udeps`, `bloat`) stays open.
* `cargo_moon_parity.py` — the self-test's plugin forward guard no longer names SMA-605 as
  pending; it records that SMA-605 closed L10 while L11's subcommand shape stays open, and its
  message now says "a `cargo_matches` arm" rather than `CARGO_INVOCATION_RE`.
* `cargo_moon_parity.py` — the `LOCK_RESOLVING_VERBS` / `CARGO_INVOCATION_RE` comment block now
  says the literal regex is one arm of three, names `cargo_matches` as the entry point every
  consumer reads, and points at A10's own sensitive variant.
* `ci/affected-graph/README.md` — the A8 and A10 bullets, and the script-following description.
* `CLAUDE.md` — the sentence "A10 shares `CARGO_INVOCATION_RE`, built from
  `LOCK_RESOLVING_VERBS`, with A8's derivation" becomes partly false.

## 8. Limitations and residuals

**R1 — the variable's NAME is the whole test for arm 1.** `BIN="$(command -v cargo)"; "$BIN"
build` stays invisible. Value resolution was measured and rejected: `VAR_ASSIGN_RE` captures
`$(` as the value of `CARGO_BIN="$( command -v cargo … )"`, so it does not reach the real shape,
and a value predicate would fire on the three variables in `ci/actionlint/run.sh` whose literal
values mention cargo — the file SMA-599 L4 already names as one edit from a spurious row.

**R2 — `export CARGO=…` separated from its tool is invisible.** Arm 2 needs the prefix and the
command in one segment.

**R3 — CLOSED (M36).** `export CARGO=/p CARGO_HOME=/q` used to report though nothing is
executed. The lookahead now skips a run of `NAME=value` assignments and demands a non-assignment
command token after them, so an assignment-only line produces no row while
`CARGO=/p CARGO_HOME=/x tool run` still does.

**R4 — `"${CARGO_BIN:-cargo}" build` is invisible to all three arms.** Arm 1 dies at the `:`,
and the literal arm misses it because `cargo}` is not `cargo\s+`. This is the idiomatic bash
default-value form for a tool path. Not fixed here; recorded because it is the most likely shape
a future author would write.

**R5 — a backtick boundary is not a start-of-command.** Arm 1's `(?:^|[\s;&|(])` excludes
`` ` ``, so ``X=`$CARGO_BIN build` `` is missed while ``X=`cargo build` `` is caught. `_tail_end`
already treats backticks as live substitutions, so the codebase considers the shape real.

**R6 — a `PATH=` prefix redirects cargo just as `CARGO=` does.** `PATH="$MY/bin:$PATH" tool` is
not covered.

**R7 — arm 1 reports prose the way the literal arm does.** The conservative rule does not strip
strings (SMA-599 L8). No such line exists today. The failure direction is a loud row and a
waiver, never a silent pass.

**R12 — the LITERAL arm still spans a newline; the two indirect arms no longer do.** MEASURED:
`cargo` + newline + `build` matches `CARGO_INVOCATION_RE`, while `"$CARGO_BIN"` + newline +
`build` does not match arm 1 and `CARGO=/p` + newline + `tool` does not match arm 2 (M34-M36).
The inconsistency is deliberate and NOT fixed here, for three reasons: the corpus holds **zero**
instances of the shape (measured across every resolved blob and every `ci/**/*.sh`); the regex
belongs to SMA-601, and changing an established assertion's behaviour needs its own measurement
of what it stops reporting; and the direction is a FALSE POSITIVE, the direction this design
accepts. Backslash continuations are unaffected either way — `_join` collapses them to a single
space before any arm sees the text, verified for all three arms.

**R8 — the resolved blob excludes `env`** (M8). A Moon task declaring `env: {CARGO: /p}` beside
a wrapper is invisible to the blob scan whatever arm 2 does.

**R9 — arm 1's blob arm inherits the per-blob vacuity the README already records.** A blob
`uv run --locked … && "$CARGO_BIN" build` passes, exactly as it does for a literal match today
(`ci/affected-graph/README.md:198-202`). This is an extension of that residual, not a new one.

**R10 — the resolver is path-insensitive, like the scan it feeds** (SMA-599 L1). Globbing
`$ECOSYSTEM` puts all three modules in every `release-parity*` task's closure, including the two
a given invocation never sources. Over-approximation only.

**R11 — A10's arm 2 may force the first `ALLOW_MISSING_CARGO_CONFIG` entry.** The allowlist is
empty by design, and SMA-599 L12 records that its stale-entry arm is therefore unasserted. Arm 2
is sensitive unconditionally over raw text, so a `CARGO=` in any comment of a followed script
would force an entry — and L12 closes at that moment. Whether this happens is settled by the
§6.3 measurement.

## 9. Acceptance criteria

| AC | Where |
|---|---|
| 1 — the decision is explicit and recorded | §4 |
| 2 — mutation-proven A8 and A10 rows | §6.1, §6.2 |
| 3 — corpus re-measured, movement explained | §3, §6.3 |
| 4 — `ci/affected-graph/run.sh` reports no expected-set movement | §6.3 |
| 5 — SMA-599's L10 updated | §7 |
