# SMA-541 — Assert every `repo:*` gate is wired into `ci.yml`'s `moon ci` target array

**Status:** revised after adversarial review (2026-08-19)
**Linear:** [SMA-541](https://linear.app/smaschek/issue/SMA-541/repo-assert-every-repo-gate-is-actually-wired-into-ciymls-moon-ci)
**Related:** SMA-525 (limitation L6, which filed this), SMA-524 / SMA-534 / SMA-546 (the sibling
assertions in `ci/affected-graph/`), SMA-542 (guards the actionlint gate's self-test invocations —
the same durability concern, and the general fix for this gate's own L6 below)

## 1. Problem

`.github/workflows/ci.yml:215` runs `moon ci` over a **hand-written** target array:

```bash
T=(:build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke …)
```

Nothing asserts that array is complete. A future `repo:*` gate can be added to `moon.yml`, be
perfectly correct, pass locally via `moon run repo:<name>`, and **never run in CI** because nobody
appended it to `T`. There is no red check. The gate simply does not exist.

That is the same silent-omission class as SMA-525 itself, one level up: a guard that is present in
the tree, believed to be running, and is not.

`ci/affected-graph/run.sh` already parses `ci.yml`, but only to assert every `moon ci` invocation
carries `--include-relations` (`assert_include_relations`). It never looks at the array's contents.

CLAUDE.md's "run the full graph like CI does" procedure (`CLAUDE.md:64-68`) enumerates the same
targets by hand a second time and can drift from `T` in either direction.

## 2. Evidence

Measured on 2026-08-19 against the pinned moon **2.3.2**, before any design decisions were made.
Everything in this section was observed, not reasoned about. E5 and E6 replace claims in the first
draft that adversarial review found wrong; they are marked.

**E1 — an unresolvable target is not an error.** The finding the whole design turns on:

```
$ moon ci :bogus-target --base origin/main --include-relations ; echo $?
Requested targets: 1 / Resolved targets: 0
0

$ moon ci :promtool :bogus-target :actionlint --base origin/main --include-relations ; echo $?
Requested targets: 3 / Resolved targets: 1
0

$ moon run :bogus-target ; echo $?
CAUTION  No tasks found. Unable to execute action pipeline. For targets :bogus-target.
1
```

`moon ci` exits **0** having resolved nothing — and, critically, also exits 0 in the **mixed** case
where real targets surround one dead entry, which is the case C2 actually protects against. So a
typo'd or renamed entry in `T` is a silent no-op on every PR. (The `moon run "${T[@]}"` fallback at
`ci.yml:222` *would* fail, but runs only on an initial push with no usable base.)

**E2 — the task inventory.** `moon query tasks --project repo` emits JSON natively; `--json` is
**not** a valid flag on moon 2.3.2 (`error: unexpected argument '--json' found`). The `repo`
project has **18** tasks. `install-hooks` is the only one with `runInCI: false`. The other 17 are
all present in `T` today, alongside 6 targets owned by other projects (`:build :test :lint :fmt
:typecheck :breaking`). `T` holds 23 entries and is currently correct — no cleanup wave.

**E3 — no task anywhere in the graph is `internal: true`.** The only task excluded from CI by its
own options is `repo:install-hooks`.

**E4 — CI-eligible tasks deliberately absent from `T` already exist — outside the `repo` project.**
`build-release` on all 13 Rust crates (`.moon/tasks/rust.yml:18-20`, no `runInCI: false`;
`run.sh:76-78` records that it "does not run in CI at all"), `contracts:generate` (invoked by its
own `ci.yml` step at line 252), and `ts:commitlint` / `ts:check-config-only` (invoked by explicit
steps at `ci.yml:191` and `:196`). This is why the forward check is scoped to the `repo` project: a
whole-graph forward check would red on `build-release` on day one.

**E5 — `runInCI: false` is documented in this repo as a *broken* way to exempt a task** *(new;
corrects the first draft's D4)*. Verbatim from `ts/moon.yml:31-32`, repeated at `:45-46`:

> Do NOT set `runInCI: false`: Moon also excludes such tasks from `moon run` whenever CI=true,
> which would make the CI gate resolve zero tasks and exit 1.

So for a task invoked by its own explicit `moon run` step, `runInCI: false` is not available. The
day a `repo:*` gate needs that treatment, D3's only sanctioned exemption does not work.

**E6 — `CLAUDE.md` is already an input to a Moon task** *(corrects the first draft's E5, which said
it was an input to none — that was a literal grep of `moon.yml`, not a resolved-inputs check)*.
`repo:actionlint` declares `inputs: ['**/*']` with project source `.`, so a CLAUDE.md-only edit
already re-runs that ~11.6s gate today. What remains true, and is what D9 rests on: `CLAUDE.md`
is **not** among `repo:affected-smoke`'s declared inputs, so without a change there the docs
assertion would be real but unreachable behind a cached PASS.

**E7 — `repo:affected-smoke` does not list `.prototools`** (`moon.yml:130-145`), though it shells
out to the proto-pinned `moon` (`.prototools:11`, `moon = "2.3.2"`) and `ci/affected-graph/README.md`
states its expected sets are "a snapshot … at the pinned moon version". Every other repo gate that
shells out to a proto-pinned binary does list it (`repo:osv`, `repo:promtool`, the three
`release-parity*`). A pre-existing gap, not one this change introduces — but this change adds two
more `moon query` calls behind it.

## 3. Design decisions

**D1 — the gate lives in `ci/affected-graph/ci_targets.py`, a Python sibling of
`cargo_moon_parity.py`, invoked from `run.sh`.** Not a bash function inside `run.sh` (three
parsers plus a fixture table — grim in bash, in a file already at 18k), and not a new `repo:*` gate
of its own (which would need its own `ci/` directory and README, would itself have to be added to
`T` and CLAUDE.md **by its own rule**, and would pay a fresh ~11.6s Moon per-task floor).
`repo:affected-smoke` already reads `ci.yml` and `moon.yml`, already runs a `--negative-control`
pass in CI, and `cargo_moon_parity.py` already establishes the shape.

**D2 — exit codes: 0 pass, 1 assertion failure, 2 infrastructure error — and rc 2 is reserved for
genuine tool failure.** `run.sh` turns rc 2 into `exit 2` of the **whole** guard, which would
destroy the diagnostics of all eight cascade cases, A1-A5 and `assert_include_relations` for that
run. So rc 2 means only: `moon` failed, its output would not parse as JSON, or its shape lacks a
key the gate needs. Every *authorial* mistake — no `T=(…)` line, two of them, a missing or
duplicated CLAUDE.md marker, an empty `repo` task set — is an **assertion failure (rc 1)** carrying
a message that says what to edit. A second full-graph example in the docs must not triage as
"re-run the job".

**D3 — the forward check (C1) is strict equality over the repo-owned partition, not a subset
test.** Partition `T`'s entries into those naming a `repo` task and those that do not; the first
set must **equal** the set of `repo` tasks with `runInCI ≠ false`.

A subset test (the first draft's design) has a one-line hole: add `options: { runInCI: false }` to
`repo:promtool` and leave `:promtool` in `T`, and the task is excluded from C1, still resolves for
C2, and leaves the docs unchanged — three green checks while `moon ci` runs nothing. That is §1's
failure statement re-openable by the guard built to close it. Strict equality reports it as an
`unexpected` entry, matching the default-deny style of `assert_case`/`assert_task_case`
(`run.sh:38-64`).

**D4 — the reverse check (C2) requires resolution to a *CI-eligible* task.** Every `:name` in `T`
must match ≥1 task named `name` **whose `runInCI ≠ false`**, anywhere in the graph. Plain
resolvability is not enough: it would let `:typecheck` pass while every task it names had been
turned off. Deliberately not strict equality against the whole graph — `T`'s six generic targets
are legitimately owned by other projects, and E4 shows a whole-graph forward check is wrong.

C1 and C2 overlap on a typo but are not redundant: C1 alone misses a stale entry for a deleted
task; C2 alone misses a new gate that was never added.

**D5 — a `T_EXEMPT` table ships empty, with a required non-empty reason string.** The first draft
refused any allowlist as premature. E4 and E5 together defeat that: CI-eligible-but-not-in-`T`
tasks already exist one project over, and the only exemption D3 sanctions is documented in this
repo as breaking `moon run` under `CI=true`. So the day a `repo:*` gate needs its own workflow step
— ordering around the codegen-drift or CODEOWNERS steps, say — the alternative to an exemption
table is deleting the assertion. The table mirrors `cargo_moon_parity.py`'s `ALLOW_NO_CARGO_BACKING`
(`:53-61`): a `{task: reason}` map, ships `{}`, and an entry with an empty reason is itself an
assertion failure.

**D6 — the docs check (C3) is an ordered, token-for-token mirror of `T`, plus the flag tail.** Not
set equality: the rule is then trivially stateable ("copy `T`") and the doc is a literal mirror a
reader can diff by eye. Line wrapping is normalised away, so reflowing the paragraph is free.

C3 **also** asserts the documented command contains `--base origin/main` and `--include-relations`.
The first draft excluded the flags on the stated grounds that `assert_include_relations` "already
owns the flag question" — it does not: that function greps `$CI_YML` only (`run.sh:126`) and never
opens CLAUDE.md. Without this, the documented command could lose `--include-relations` and silently
under-build, which is the very behaviour §1 cites as the reason to check the docs at all.

**D7 — the docs command is delimited by explicit HTML-comment markers, not recognised by prose
shape.** The first draft selected an inline-code span by three coincident substrings ("starts with
`moon ci`" ∧ contains `--include-relations` ∧ contains a `:token`). That is unique against today's
file but fragile in both directions against ordinary doc edits: converting the 5-line command to a
fenced ```` ```bash ```` block — CLAUDE.md currently has no fenced blocks and ~250 backticks —
zero-matches it, and merging or extending the two neighbouring gotchas that already contain
`` `moon ci :build` `` (`CLAUDE.md:13`) and `` `moon ci --include-relations` `` (`:89`) could
two-match it. The blast radius of an innocuous documentation edit would be the repo's only required
status check.

Instead, `<!-- ci-targets:begin -->` / `<!-- ci-targets:end -->` wrap the command, and the gate
takes every `:`-prefixed token between them. The contract becomes visible to whoever edits the
file. A missing, duplicated, or inverted marker pair is an assertion failure (rc 1) naming the
marker. This also disambiguates the illustrative gate list in the same bullet (`` `:deny` ``,
`` `:osv` ``, …), which sits outside the markers and is reworded to lead with "e.g.".

**D8 — one `moon query tasks` call, filtered by project id in Python.** Not `--project repo`:
moon's query filters are regex-based and unanchored, so a future project named e.g.
`paigasus-repo-ts` would silently join the "repo task set" and false-red C1. One subprocess serves
both the repo task set and the whole-graph name set. `options.runInCI` absent — or `options` absent
entirely — means **eligible** (default toward inclusion, so a shape change cannot silently exempt a
gate). If **no** task in the entire output carries an `options` key, that is a shape change and an
rc 2 infrastructure error, mirroring `cargo_moon_parity.py`'s treatment of a missing `inputFiles`.

**D9 — `CLAUDE.md` and `.prototools` join `repo:affected-smoke`'s `inputs`.** `CLAUDE.md` per E6,
without which C3 is unreachable behind a cached PASS. `.prototools` per E7, without which a
moon-bump PR touching only that file leaves this whole guard — whose expected sets are explicitly
version-pinned snapshots — serving a cached PASS. Cost: any CLAUDE.md edit now re-runs
`repo:affected-smoke`. Accepted; note E6 means such edits already re-run `repo:actionlint`.

**D10 — every entry of `T` must be the `:name` shorthand; a project-scoped entry fails loudly.**
Moon accepts both `:promtool` and `repo:promtool`. A naive `:`-token extraction would pull
`:promtool` out of `repo:promtool` and normalise away the very thing worth noticing, and a
whitespace-anchored one would skip it silently — either way the array would contain something the
gate never examined. So `T` is **whitespace-split and every token classified**: `:name` is a
target, anything else is an assertion failure naming it. Today's `T` is uniformly shorthand (E2). If
a scoped entry is ever wanted, the gate reds and the author extends the parser deliberately.

**D11 — the two parsers are pure functions with their own fixture tables.** `parse_t(text)` and
`parse_doc_targets(text)` take text and return token lists, so `--self-test` can exercise the
parsing itself rather than only the comparison logic. This is the lesson `ci/actionlint/run.sh:265`
already records — "hand-rolled YAML parsing … is exactly the kind of thing that silently does the
wrong thing" — backed there by ~35 extractor fixtures. A parser bug is the one failure this gate
cannot self-detect: a total match failure hits the rc-1 path, but a *partial* mis-parse is silent.

**D12 — `T` is matched with `^[ \t]*T=\((.*?)\)[ \t]*$` under `re.MULTILINE`, and any other
`T`-assignment line is an assertion failure.** `\s` matches newlines in Python, so `\)\s*$` could
anchor at a later line's end and quietly accept a multi-line array; `[ \t]` cannot. Separately, a
future `T+=(:new-gate)` append (or a second conditional array) would leave its entries unexamined
by C2 while C1 still passed — so any line matching `^[ \t]*T[ \t]*\+?=` that is not the single
canonical assignment reds the gate.

**D13 — the gate asserts its own two call sites (C4).** D1's placement means C1 does not cover this
gate: its execution depends on one `assert_ci_targets` line in `run_suite` and one
`ci_targets.py --self-test` line in the `--negative-control` branch, and deleting either leaves
everything green. C4 asserts `run.sh` contains both, in the manner of the
`redis-connect-single-site` / `iam-docker-policy-single-site` marker checks. This is a **partial**
mitigation, not a closure — deleting the `assert_ci_targets` call removes C4 along with it. See L6.

## 4. Components

### `ci/affected-graph/ci_targets.py`

**Inputs** — three, resolved from the repo root the way `run.sh` does:

| # | Source | Used for |
|---|---|---|
| 1 | `moon query tasks` (one call, D8) | `repo`'s `{name → runInCI}`, and every task name in the graph with its eligibility |
| 2 | `.github/workflows/ci.yml` | the sole `T=(…)` line (D12) |
| 3 | `CLAUDE.md` | the marker-delimited command (D7) |

**Checks:**

- **C1 — forward, strict equality over the repo partition (D3).** `{T entries naming a repo task}`
  == `{repo tasks with runInCI ≠ false}` ∖ `T_EXEMPT`. Failure lists `missing` and `unexpected`
  separately, in the wording `assert_case` already uses, and names `ci.yml`'s `T` as the fix site.
- **C2 — reverse, CI-eligible resolution (D4).** Every `:name` in `T` matches ≥1 CI-eligible task
  in the graph. Failure names each dead or fully-disabled entry.
- **C3 — docs mirror (D6).** The marker-delimited target list equals `T` token-for-token in order,
  and the delimited region contains `--base origin/main` and `--include-relations`. Failure reports
  the first divergence by position and prints both lists.
- **C4 — self-invocation (D13).** `run.sh` contains both the `assert_ci_targets` call and the
  `--self-test` call.

**Anti-vacuity floors**, all rc 1 unless noted:

- the parsed CI-eligible `repo` set must contain a hardcoded minimum — `affected-smoke`,
  `publish-metadata`, `promtool` — so a project filter that starts matching the wrong thing, or a
  moon output shape change, fails loudly instead of comparing two empty sets (the
  `REQUIRED_FFI_TASKS` precedent, `cargo_moon_parity.py:95-103`)
- `T=(…)` must match exactly one line; no other `T`-assignment line may exist (D12)
- the CLAUDE.md marker pair must appear exactly once, in order, non-empty (D7)
- every `T_EXEMPT` entry must carry a non-empty reason (D5)
- `moon` failing, non-JSON output, or an output in which no task carries `options` → **rc 2** (D8)

### `ci/affected-graph/run.sh`

- New `assert_ci_targets()` shelling out to `ci_targets.py`, folding rc 0/1/2 exactly as
  `assert_cargo_moon_parity` does. Called **last** in `run_suite`, so that even a genuine rc-2
  abort cannot suppress the other assertions' diagnostics.
- The `--negative-control` branch gains `python3 "$HERE/ci_targets.py" --self-test || NEG_RC=1`.

### `moon.yml`

`repo:affected-smoke` gains `CLAUDE.md` and `.prototools` to its `inputs` (D9), each with a comment
naming its reason. No other change: the task already runs `--negative-control` then the real suite.

### `CLAUDE.md`

The `:64-68` bullet is restructured so the command sits alone between the two markers, and its
illustrative gate list is reworded to lead with "e.g." (D7). A line is added to the gotcha list
noting that a new `repo:*` gate reds `:affected-smoke` until it is in **both** `T` and the
documented command — mirroring the existing "a new Rust crate reds `:affected-smoke`" note.

### `ci/affected-graph/README.md`

A bullet describing C1-C4, the `T_EXEMPT` contract, and the marker contract.

## 5. Testing

`--self-test` drives the **parsers** and the **check functions** against in-memory fixtures, so no
verdict depends on the tree happening to be aligned.

**Parser fixtures (D11).** For `parse_t`: a trailing comment after `)`; `T+=( … )` on a second
line; two `T=(…)` lines; a `T=(` inside a YAML `#` comment; an empty `T=()`; a `repo:promtool`
token; a token with no leading colon. For `parse_doc_targets`: a span wrapped across 5 lines; a
missing marker; a duplicated marker; inverted marker order; an empty region; targets plus the flag
tail.

**Check fixtures:**

| Fixture | Expected |
|---|---|
| a `repo` task absent from `T` | C1 **red** (`missing`), naming it |
| `install-hooks` (`runInCI: false`) absent from `T` | **green** — AC #3, asserted not assumed |
| a task flipped to `runInCI: false` but left in `T` | C1 **red** (`unexpected`) — the D3 hole |
| a task in `T_EXEMPT` with a reason, absent from `T` | **green** |
| a `T_EXEMPT` entry with an empty reason | **red** |
| a dead `:ghost` entry in `T` | C2 **red**, naming it |
| a `:name` resolving only to `runInCI: false` tasks | C2 **red** — the D4 hole |
| a project-scoped `repo:promtool` entry in `T` | **red** (D10) — never silently ignored |
| doc missing a target / doc in the wrong order | C3 **red** on each |
| doc missing `--include-relations` | C3 **red** (D6) |
| `run.sh` text missing either call site | C4 **red** |
| a repo task set that omits a floor member | **red** |
| a task set in which no task carries `options` | **rc 2** |
| everything aligned | **green** — catches a permanently-red harness |

Beyond the table, verification is by mutation against the real tree, run and recorded rather than
assumed: add a throwaway `repo:` task and confirm the gate names it; flip an existing gate to
`runInCI: false` leaving `T` untouched and confirm C1 fires; mistype one `T` entry and confirm both
C1 and C2 fire; delete one target from CLAUDE.md and confirm C3 fires; then confirm the unmutated
tree is green. The wall-clock cost of the added `moon query` call is measured and recorded in the
README, since D1 rejects a standalone task partly on cost grounds.

## 6. Limitations

- **L1 — `T` must stay a single-line bash array.** Reformatting it across lines reds the gate
  (rc 1, with a message saying so). A constraint on `ci.yml`'s formatting, made explicit.
- **L2 — a project-scoped entry in `T` is rejected, not supported** (D10).
- **L3 — the gate asserts membership, not execution.** A target present in `T` and resolvable can
  still do nothing useful — a `repo:*` task whose `inputs` never match any file runs green forever.
  *This is **not** covered elsewhere in this script*, contrary to the first draft's claim: A4
  iterates Cargo crates, A5 derives FFI tasks, and `lockfile->all-lint` is a Rust-task case — none
  looks at a `repo:*` task's inputs. Concretely, moving `ops/` would leave `repo:promtool`'s
  `inputs` matching nothing. A follow-up issue should assert every `repo` task's resolved inputs
  are non-empty and match ≥1 tracked file (`pattern_verdict`'s `dead` verdict in
  `ci/actionlint/run.sh:960` is the precedent). Out of scope here — that is an inputs assertion,
  not a wiring one.
- **L4 — other workflows are out of scope.** `security-scan.yml` runs `osv-scanner` directly;
  nothing here asserts anything about targets outside `ci.yml`'s `T`.
- **L5 — C3 makes the documented command *consistent* with `T`, not *sufficient* to reproduce CI.**
  `ci.yml` separately runs `ts:commitlint`, `ts:check-config-only`, `contracts:generate` plus its
  drift diff, and the CODEOWNERS sync. The documented command never covered those and still does
  not.
- **L6 — this gate's own invocation is only partially guarded.** C4 catches deleting one of the two
  call sites; deleting the `assert_ci_targets` call removes C4 with it. SMA-542 is the general fix
  for this class and should be extended to cover `ci/affected-graph/run.sh`'s call sites. It is
  **not** a hard dependency — this spec ships C4 and does not block on it.
- **L7 — `CLAUDE.md` is the only doc checked.** `CONTRIBUTING.md` and the READMEs could grow their
  own copy of the command uncaught.
- **L8 — break-glass.** The fix path for a red is always "edit `T` and/or CLAUDE.md", with one
  exception: a parser break (L1, a marker edit) reds without any target being wrong. There is no
  warn-only mode by design; the escape for a legitimately-exempt task is `T_EXEMPT` (D5).

## 7. Acceptance criteria

| Issue AC | Covered by |
|---|---|
| Adding a `repo:*` task to `moon.yml` without adding it to `T` fails the gate, naming the task | C1 |
| The check carries its own control proving it can fail, verified rather than assumed | `--self-test` (parser + check fixtures, §5), run by `repo:affected-smoke`'s existing `--negative-control` pass; plus the mutation verification in §5 |
| `install-hooks` (and anything else with `runInCI: false`) does not trip it | D3 + its dedicated fixture row |
| CLAUDE.md's documented full-graph command is kept honest by the same check | C3, made reachable by D9 |

## 8. Changelog — adversarial review (2026-08-19)

Folded in: strict equality over the repo partition, closing a one-line hole where flipping a gate
to `runInCI: false` while leaving it in `T` passed all three checks (D3); CI-eligible resolution in
C2 (D4); parsers as fixtured pure functions (D11); rc 2 narrowed to genuine tool failure so an
authorial mistake cannot abort the whole affected-graph suite (D2); marker-delimited docs command
replacing prose-shape matching (D7); an empty-but-present `T_EXEMPT` (D5), after evidence that
`runInCI: false` is documented here as a broken exemption (E5) and that CI-eligible tasks outside
`T` already exist (E4); the flag tail added to C3 after the first draft's rationale for omitting it
proved false (D6); `.prototools` added to the task inputs (E7, D9); a self-invocation check (D13);
single anchored task query (D8); `[ \t]` anchoring and `T+=` rejection (D12); floors for the parsed
task set (§4); corrected E5→E6 after the original claim was found to be a literal grep rather than
a resolved-inputs check.

Not folded in: a fourth check asserting every `repo` task's inputs match ≥1 tracked file. Real and
uncovered, but an inputs assertion rather than a wiring one, and materially wider than this issue —
recorded as L3 with a follow-up instead.
