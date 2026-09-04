# SMA-608 — Validate `rs/release-plz.toml`'s section shapes instead of skipping past them

**Status:** revised after adversarial review (2026-09-04)
**Linear:** [SMA-608](https://linear.app/smaschek/issue/SMA-608/ci-release-planpy-silently-ignores-malformed-release-plztoml-section)
**Related:** **SMA-603** (built `ci/release-plan/`; this is its logic), SMA-539 (raised the finding
via CodeRabbit on PR 206 and deferred it — that branch's only touch to the file was a mechanical
`Inconclusive` -> `InconclusiveError` rename demanded by a newly-enabled Ruff `N818`)

## 1. Problem

`assert_default_tag_format` in `ci/release-plan/release_plan.py` reads two sections of
`rs/release-plz.toml` without validating their TOML shape:

```python
if "git_tag_name" in (cfg.get("workspace") or {}):
    ...
for pkg in cfg.get("package") or []:
    if isinstance(pkg, dict) and "git_tag_name" in pkg:
```

**Three** malformed-but-TOML-valid shapes bypass the assertion instead of tripping it. The issue
names two; the third was found during adversarial review and is MEASURED in E1:

* `workspace = []` — an empty list is falsy, so `or {}` substitutes `{}` and the membership test
  is vacuously false. *(falsy substitution)*
* `[[workspace]]` — workspace as an array of tables. `[{...}]` is **truthy**, so `or {}` does not
  substitute; `"git_tag_name" in [{"git_tag_name": ...}]` compares against the dict as an element
  and is `False`. *(truthy wrong container)*
* `package = { ... }` — a table rather than an array of tables. Iterating a dict yields its keys
  as **strings**, so the `isinstance(pkg, dict)` guard skips every one. *(type-blind iteration)*

Either way the guard passes having asserted nothing, and `tag_for()`'s assumption that release-plz
uses its default `<package>-v<version>` tag format goes unchecked.

**Why it is worth closing despite a bounded blast radius.** The issue argues release-plz itself
rejects these shapes, so such a config fails loudly at the release step rather than silently
cutting wrong tags. **That claim is UNMEASURED** — it is repeated from the Linear issue and was not
verified against the pinned release-plz 0.3.158 here. It is not load-bearing for this design, which
is justified instead by the invariant: SMA-603 was built so that an inconclusive decision must
BUILD, never skip. A guard that cannot distinguish "the config is fine" from "I could not read the
config" is the shape that issue exists to prevent, and this one currently cannot. E7 below finds a
shape whose direction is a genuine SKIP, which removes the need to rely on the unmeasured claim.

## 2. Evidence

Read off the tree at `b5e0667` (`origin/main`). E1, E7 and E8 were MEASURED by execution; the rest
are properties of the code as written. §7 lists what implementation must still measure.

**E1 — the three bypasses are real and mechanically distinct.** MEASURED:
`tomllib.loads("[[workspace]]\ngit_tag_name = 'x'\n")` yields `[{'git_tag_name': 'x'}]`, and
`'git_tag_name' in [{'git_tag_name': 'x'}]` is `False`. The falsy and type-blind cases follow by
inspection. A fix addressing only one leaves the others.

**E2 — the same blind spot exists a second time, in `releasable_packages`.** Ten lines below the
call, the entry map re-implements the identical filter:

```python
entries = {p["name"]: p for p in (cfg.get("package") or [])
           if isinstance(p, dict) and isinstance(p.get("name"), str)}
```

With `package = { ... }` this yields an empty `entries`, so every crate reads as releasable and its
missing tag makes the decision BUILD. That direction is fail-safe, so E2 is not a second defect —
but it is the same predicate, written twice, both times wrong.

**E3 — the fix falsifies a measurement this file cites in FOUR places, not five.** `rg TypeError ci/`
finds exactly: `release_plan.py:212` (`run()`), `:409` (`_malformed_config_asserts_three`), `:479`
(`_assert_repo`), and `README.md:142`. Each justifies a deliberately-broad `except Exception` by
citing `workspace = 3` raising a bare `TypeError` out of `assert_default_tag_format`'s membership
test. After this change that shape raises `InconclusiveError`. The broad catches must **stay** —
E8 is why — but the rationale becomes a false claim, and this repo treats a stale measurement as a
defect. (`README.md:76` mentions `assert_default_tag_format` without the `TypeError` claim; it
needs a different correction, §3.9.)

**E4 — the collection-layer rows have no arity floor.** `self_test()` floors `FIXTURES` at 8 rows
and says why. The six collection-layer helpers below it are an inline tuple with no equivalent.
Deleting a helper from that tuple reds nothing: check 11's `--fixture-count` floor
(`ci/actionlint/run.sh:4583`) counts `FIXTURES` only.

**E5 — the collection-layer loop is deletable in silence.** `run.sh`'s row 7 states the reasoning
for the `FIXTURES` loop verbatim — "with it gone, `--self-test` still returns 0 and every other
control here still passes" — and closes it with a mutation row. The collection-layer loop has the
identical property and no such row.

**E6 — the real config is unaffected.** `rs/release-plz.toml` declares `[workspace]` as a table and
thirteen `[[package]]` entries as an array of tables. AC4 holds by construction, still MEASURED per
M4.

**E7 — a duplicate `[[package]] name` produces a SKIP, and nothing catches it at runtime.**
MEASURED: `{p["name"]: p for p in entries}` keeps the **last** entry for a repeated name — two
`[[package]]` blocks both named `k`, the second carrying `release = false`, yield
`entries['k'] == {'name': 'k', 'release': False}`. So a duplicate entry for `paigasus-kernel`
carrying `release = false` drops that crate from `out` in `releasable_packages`; no tag is ever
demanded for it; and if the other two tags exist, `decide` returns `True` — **a real release is
skipped**. `crate_manifests:161-163` raises on duplicate *manifests*; nothing raises on duplicate
*release-plz entries*. `--assert`'s `EXPECTED_RELEASABLE` pin would catch it on a PR, but the
runtime path deliberately never consults that set (`README.md:68-71`), so no runtime control exists.
This is the only shape found in this review whose direction is SKIP.

**E8 — after the naive fix, NO fixture exercises either broad `except Exception`.**
`_malformed_config_asserts_three` exists because `workspace = 3` raised an untyped `TypeError`.
Once `config_sections` types that shape, walking every post-change fixture leaves zero producing a
non-`InconclusiveError` through collection. Narrowing `_assert_repo`'s catch (`:491`) back to
`except InconclusiveError` would then leave `--self-test` **green**. `run()`'s catch (`:220`) is
already untested today: no fixture calls `run()` against a broken tree, and `run.sh` rows 3/4 point
it at well-formed synthetic trees. So the fix as first drafted *removed a live control and replaced
its docstring rather than its coverage* — the exact failure class this spec is about.

**E9 — three stale-pointer defects in the files being edited.** (a) `release_plan.py:437-439` says
the `FIXTURES` floor is "deliberately duplicated by a second, independent floor in
`ci/release-plan/run.sh`'s own negative control". It is not — `run.sh` has no count floor in any of
rows 1-7; the real twin is `ci/actionlint/run.sh:4583`. (b) `_tag_name_override_is_inconclusive`'s
docstring (`:319-326`) quotes a message the code cannot emit: it says `crate_manifests` raises
`"no crate manifests under .../crates — the tree moved"`, but the emitted string is
`f"no crate manifests under {rs_root} …"` (no `/crates` segment) — and with that fixture's
Cargo.toml-less tree the branch is unreachable anyway, because `crate_manifests` calls
`workspace_members` -> `load_toml(rs_root / "Cargo.toml")` first and dies at
`cannot read …/rs/Cargo.toml`. (c) `ci/actionlint/run.sh:2140-2141` says "those **nine** lines",
a third prose site counting `RELEASE_PLAN_SH_CALL_SITES`.

## 3. Design

### 3.1 One validator, consumed once

A new pure function validates and returns both sections:

```python
def config_sections(cfg: dict) -> tuple[dict, list[dict]]:
    """Validate rs/release-plz.toml's two sections and return them as ([workspace], [[package]])."""
```

* `workspace` absent -> `{}`. Present and not a `dict` -> `InconclusiveError`.
* `package` absent -> `[]`. Present and not a `list` -> `InconclusiveError`. A `list` holding a
  non-`dict` element -> `InconclusiveError` naming the element's index.
* A `name` repeated across entries -> `InconclusiveError` naming it (E7).

`cfg.get(key, default)` replaces `cfg.get(key) or default` **for these two `release-plz.toml`
sections only**. That substitution is the fix's core: TOML has no null, so a present key always
carries a non-None value, and the explicit default routes `workspace = []` to the `isinstance`
check instead of substituting `{}` past it. The file's two other `… or {}` sites (`:158`, `:185`)
read a **Cargo.toml**, not `release-plz.toml`; they are **not** touched, and `:158`'s behaviour is
load-bearing for the E8 replacement fixture (§3.4 row 11).

**The list check and the element loop must be two separate statements**, not one
`isinstance(packages, list) and all(...)`. M2 neuters the first; if they are fused, neutering it
also disables the element check and the mutation lands on a different failure than the one M2
names.

`assert_default_tag_format` then takes the already-validated sections and drops its own guards:

```python
def assert_default_tag_format(workspace: dict, packages: list[dict]) -> None:
    if "git_tag_name" in workspace: ...
    for pkg in packages:
        if "git_tag_name" in pkg: ...
```

and `releasable_packages` becomes:

```python
cfg = load_toml(rs_root / "release-plz.toml")
workspace, package_entries = config_sections(cfg)
assert_default_tag_format(workspace, package_entries)
entries = {p["name"]: p for p in package_entries
           if isinstance(p, dict) and isinstance(p.get("name"), str)}
```

**`isinstance(p, dict)` is deliberately RETAINED, reversing the first draft.** E2 called it
duplication worth removing. Removing it is what makes M3 fail: MEASURED, with the element check
neutered and `package = ["a"]`, `"git_tag_name" in "a"` is `False` (string containment, no raise)
and the comprehension then evaluates `"a".get` -> `AttributeError: 'str' object has no attribute
'get'`. `self_test()` calls each helper bare (`:461`), so that escapes and the interpreter exits
**1 with a traceback** — breaking `README.md:139-141`'s "0, 2 or 3, never 1" contract and routing
a repo-shape failure onto `die_infra` (2). The guard costs one `isinstance` and keeps the mutated
failure **typed**, so the fixture reports its designed wrong-reason string. The consolidation this
spec delivers is of the *validation predicate*, which was genuinely written twice and wrongly both
times; the comprehension's type guard is a belt, and §3.9 says so in the code.

**Deliberately out of scope: a nameless `[[package]]` entry.** The surviving
`isinstance(p.get("name"), str)` filter still drops an entry with no string `name`. Adversarial
review argued this is inconsistent with `workspace_members`' loud refusal of `[workspace] exclude`
(`:121-130`), which rejects the same "intent silently lost" shape. The counter-argument is
direction: a dropped entry only ever *grows* the demanded-tag set, so it BUILDS, whereas a dropped
`exclude` makes the skip permanently unreachable. It is left out because it is outside the AC and
fail-safe — **flagged at the gate as a live question, not settled.**

### 3.2 Markers, and the substrings fixtures match on

Each raise carries a marker no other raise in this module produces, and each names the file so a
`run()` warning line stays diagnosable:

| Shape | Emitted message contains | Fixture matches on |
|---|---|---|
| non-table `workspace` | `rs/release-plz.toml's [workspace] is not a table` | `[workspace] is not a table` |
| non-array `package` | `rs/release-plz.toml's [[package]] is not an array of tables` | `[[package]] is not an array of tables` |
| non-table element | `rs/release-plz.toml's [[package]] entry 0 is not a table` | `[[package]] entry` |
| duplicate name | `rs/release-plz.toml declares [[package]] name 'k' twice` | `declares [[package]] name` |

The **matched substrings** are pinned here, not just the emitted strings: matching row 3 on the
obvious `"is not a table"` would accept a `[workspace]`-caused error as its own. Distinctness is
load-bearing — `_tag_name_override_is_inconclusive`'s docstring records the MEASURED anti-pattern
where a bare `except InconclusiveError` kept a helper passing on an unrelated raise further down.

### 3.3 Fail-safe direction is unchanged

Every new raise is `InconclusiveError`. `run()` converts it to `nothing_to_release=false` (BUILD);
`_assert_repo()` converts it to exit 3. E7's duplicate-name raise **removes** a SKIP path. No shape
reachable through `config_sections` can produce a skip.

### 3.4 Fixtures: six new rows, taking the tuple to twelve

* **7. `_workspace_not_a_table_is_inconclusive`** — `workspace = []`, the falsy shape.
  (`workspace = 3`, the truthy-scalar shape, stays covered by `_malformed_config_asserts_three`,
  which after this change exercises the same validator.)
* **8. `_workspace_array_of_tables_is_inconclusive`** — `[[workspace]]`, E1's truthy wrong
  container.
* **9. `_package_not_an_array_of_tables_is_inconclusive`** — `package = { name = "a" }`.
* **10. `_package_entry_not_a_table_is_inconclusive`** — `package = ["a"]`.
* **11. `_duplicate_package_name_is_inconclusive`** — E7's SKIP path.
* **12. `_untyped_collection_failure_asserts_three`** — E8's replacement control. Tree: valid
  `rs/Cargo.toml` and `rs/release-plz.toml`, plus `rs/crates/libs/a/Cargo.toml` containing
  `package = 3`. MEASURED: `crate_manifests:157` reads `.get("package") or {}` -> `3`, then
  `:158` calls `3.get("name")` -> `AttributeError: 'int' object has no attribute 'get'`, which
  **only** the broad catch converts. Asserts `_assert_repo` returns 3. This restores the coverage
  `_malformed_config_asserts_three` loses, and it is what keeps §4's "neither broad catch is
  narrowed" enforceable rather than merely promised.

**Each fixture's fall-through, stated correctly (E9b).** A crate-less tree does **not** reach
`crate_manifests`' "no crate manifests" branch. It dies at `load_toml`'s
`cannot read …/rs/Cargo.toml`. That is still a *different* error, so the wrong-reason property the
fixtures need holds — but an implementer following the first draft's wording might add an
`rs/Cargo.toml` chasing the named branch and change what M1-M3 observe. Each fixture builds a tree
with **no `rs/Cargo.toml` and no `rs/crates/`**, and the expected fall-through is the `cannot read`
message.

**`run()`'s untested broad catch (E8) stays untested.** Closing it needs a row asserting
`run(broken_tree, "push") == (False, "inconclusive (AttributeError: …) — build")`. That is cheap
and in-theme, but it is a *new* control rather than a restoration, so it is **flagged at the gate**
rather than assumed.

### 3.5 `self_test()`'s collection loop must not be able to exit 1

Independently of any mutation, `err = fn()` at `:461` lets any helper bug escape as a traceback at
exit 1, violating the documented contract. The loop wraps each call:

```python
try:
    err = fn()
except Exception as exc:
    err = f"raised {type(exc).__name__}: {exc}"
```

This is a genuine hardening, not mutation scaffolding: it makes the never-1 contract true of the
self-test path for *any* future helper.

### 3.6 An arity floor on the collection rows — with a twin (E4, and review MAJOR 3)

The inline tuple moves to a module-level constant, placed **immediately before `self_test()`** and
not beside `FIXTURES` — the six helpers it references are defined at `:252-430`, so a constant next
to `FIXTURES` (`:228`) raises `NameError` at import. Its `Callable` annotation must be imported
under `if TYPE_CHECKING:`; with `from __future__ import annotations` already at `:21`, a plain
`from collections.abc import Callable` used only in an annotation trips ruff `TC003` under
`repo:ruff-ci` (`py/pyproject.toml:25`).

`self_test()` floors it at **10** against twelve actual rows. Critically, the floor gets the twin
the `FIXTURES` floor has and the first draft's did not: a new `--collection-count` flag prints the
tuple's length, and `release_plan_self_test` in `ci/actionlint/run.sh` floors it at 10 alongside the
existing `--fixture-count` check. **Do not widen `--fixture-count`** — its consumer at
`ci/actionlint/run.sh:4578` validates the output is a single integer.

### 3.7 Row 8 of the negative control (E5)

`run.sh` gains a row mirroring row 7: it neuters the `workspace` shape check in a COPY of
`release_plan.py`, asserts via `cmp -s` that the mutation changed the file, and asserts the
mutant's `--self-test` fails.

**The mutation is a condition neutering, specified exactly** —
`s/if not isinstance(workspace, dict):/if False and not isinstance(workspace, dict):/` — verified
to occur exactly once in `release_plan.py`. It must not delete the `raise`: every
`raise InconclusiveError(...)` in this file spans two physical lines (`:97-98`, `:101-102`), so a
line-deleting `sed` leaves an empty `if` body -> `IndentationError` -> the mutant exits **1**, and
row 8 would red with a diagnostic pointing at the wrong thing while `cmp -s` passed.

**The row asserts on stderr, not on rc alone.** Adversarial review found that rc 3 is satisfiable
by §3.6's own floor: delete two rows from the tuple and the floor fires, `self_test()` returns 3
*for the floor*, and an rc-only assertion goes green while the neutered workspace check went
undetected — the two new controls covering for each other's absence. The row therefore greps the
mutant's stderr for the specific `FAIL` label of
`_workspace_not_a_table_is_inconclusive`, exactly as rows 3/4 grep for a specific verdict line
rather than trusting an exit code.

The row's guard uses a local named `mut8_rc`, distinct from row 7's `mut_rc` — the rename
`ci_targets.py`'s `RUFF_SH_CALL_SITES` comment records as necessary to stop a pinned guard line
being satisfied by an unrelated identical line in the same file.

### 3.8 Registry obligations

`RELEASE_PLAN_SH_CALL_SITES` gains a tenth entry: row 8's assertion line
`if [ "$mut8_rc" != "3" ]; then`. Implementation must verify it occurs exactly once in `run.sh`, as
every existing entry was (`ci_targets.py:1048`).

**Three** prose sites count that tuple and must move with it (E9c corrects the first draft's "two"):

1. `ci/affected-graph/ci_targets.py:1019-1039` — the enumeration of items 1-9.
2. `moon.yml:216-217` — "pins nine load-bearing lines".
3. `ci/actionlint/run.sh:2140-2141` — "the PR deleting those nine lines".

There is **no** numeric length assertion on the tuple anywhere: `len(RELEASE_PLAN_SH_CALL_SITES)`
appears in zero files, and both the fixture at `ci_targets.py:2338-2340` and the deletion battery at
`:2714` derive from the tuple, so a tenth entry is covered automatically. The flip side, recorded
and **not** fixed here: emptying that tuple to `()` makes the check report zero missing sites and
the battery iterate zero times — E4's defect, in the registry this spec extends, shared by all five
`*_SH_CALL_SITES` tuples. Fixing all five is its own issue.

Reachability needs no change: `moon.yml` lists `ci/release-plan/**/*` among `repo:affected-smoke`'s
inputs and `T_AFFECTED_SMOKE_REQUIRED_INPUTS` floors that entry.

### 3.9 Documentation corrections

* The four `TypeError` citations (E3) keep their broad-catch rationale but stop citing a shape that
  is now typed: the catch exists for the **residual**, `workspace = 3` is named as the historical
  motivation and explicitly marked as no longer producing a `TypeError`, and row 12 is named as the
  control that now exercises the catch.
* `release_plan.py:437-439` (E9a) — correct the `FIXTURES` floor's twin to
  `ci/actionlint/run.sh:4583`.
* `release_plan.py:319-326` (E9b) — correct the quoted message and the claimed fall-through.
* `README.md:76` — `assert_default_tag_format()` no longer reads the file; state that
  `config_sections()` does, and that a malformed `[workspace]`/`[[package]]` is now **refused**
  rather than ignored. This is the one place the README gains new behaviour rather than a
  correction.
* `README.md:86` — "six collection-layer rows" -> twelve, enumerated.
* `README.md`'s `--negative-control` "seven rows" -> eight, describing row 8.
* The retained `isinstance(p, dict)` (§3.1) carries an inline comment naming the MEASURED
  `AttributeError` it prevents, so a future reader does not "simplify" it away.

Historical plan documents are treated as frozen:
`docs/superpowers/plans/2026-08-29-sma-603-release-plan-job.md:208` carries the old
`assert_default_tag_format(cfg: dict)` signature and is **not** edited. No live caller, test, doc or
pin references that signature — the only caller is `releasable_packages:179`.

## 4. What this does NOT do

* It does not parse a custom `git_tag_name` template. The checker still refuses and builds.
* It does not model `[workspace] exclude`.
* It does not make a nameless `[[package]]` entry an error (§3.1) — gate question.
* It does not add a `run()`-path fixture (§3.4) — gate question.
* It does not narrow either broad `except Exception`, and after row 12 that is *enforced* rather
  than merely stated.
* It does not floor the other four `*_SH_CALL_SITES` tuples (§3.8).

## 5. Acceptance criteria mapping

| AC | Where |
|---|---|
| 1. non-table `workspace` raises `InconclusiveError` | §3.1; fixtures 7 and 8 |
| 2. non-array-of-tables `package` raises `InconclusiveError` | §3.1; fixtures 9 and 10 |
| 3. both have `--self-test` fixtures, proven to red by mutation | §3.4 (fixtures), §3.7 (continuous proof in CI), M1-M3 (one-time measurements) |
| 4. the real `rs/release-plz.toml` still passes unchanged | E6, verified by M4 |

Beyond the AC, justified by the fail-safe invariant: E7's SKIP path (fixture 11) and E8's lost
broad-catch coverage (fixture 12).

## 6. Risks

**R1 — a fixture passes for the wrong reason.** Mitigated by §3.2's pinned match substrings and
§3.4's corrected fall-through. M1-M3 prove it rather than assume it.

**R2 — row 8's `sed` target drifts.** Mitigated by the `cmp -s` vacuity assertion and by §3.7
specifying the exact expression.

**R3 — the tenth `RELEASE_PLAN_SH_CALL_SITES` entry is not unique in `run.sh`.** Mitigated by the
`mut8_rc` rename; implementation must verify.

**R4 — row 8 costs a seventh `uv run` in `negative_control()`.** `release_plan_self_test` runs
inside check 9's battery across 14 mutants plus the unmutated control, so it fires ~15x per full
`repo:actionlint` run. `ci/actionlint/README.md:698-704` records SMA-603's own addition at +0.5s
`--self-test` / +2.7s full gate, and `:714-719` states the subprocess count is load-bearing.
M7 re-measures.

## 7. Measurements required during implementation

Each must be run and recorded; none may be argued from the code.

* **M1** — neuter the `workspace` shape check; confirm `_workspace_not_a_table_is_inconclusive`
  reports its wrong-reason string and `--self-test` exits 3 with that `FAIL` label on stderr.
* **M2** — neuter the `package` list check (separate statement per §3.1); same, for fixture 9.
* **M3** — neuter the element check; same, for fixture 10. With `isinstance(p, dict)` retained the
  expected outcome is a clean wrong-reason report, **not** the `AttributeError` traceback the first
  draft would have produced — confirm the traceback is gone.
* **M4** — `bash ci/release-plan/run.sh --assert` against the real repository exits 0 (AC4).
* **M5** — `--self-test` and `--negative-control` both pass, row 8 included.
* **M6** — `repo:actionlint` (check 11) and `repo:affected-smoke` (the pin) both green.
* **M7** — re-measure `--self-test` and full-gate wall time; update `ci/actionlint/README.md`'s
  subprocess count and timings.
* **M8** — narrow `_assert_repo`'s `except Exception` to `except InconclusiveError` and confirm
  fixture 12 reds. This is what proves E8's hole is actually closed rather than just described.
