# SMA-608 — Validate `rs/release-plz.toml`'s section shapes instead of skipping past them

**Status:** draft (2026-09-04)
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

Two malformed-but-TOML-valid shapes bypass the assertion instead of tripping it:

* `workspace = []` — an empty list is falsy, so `or {}` substitutes `{}` and the membership test
  is vacuously false.
* `package = { ... }` — a table rather than an array of tables. Iterating a dict yields its keys
  as **strings**, so the `isinstance(pkg, dict)` guard skips every one.

Either way the guard passes having asserted nothing, and `tag_for()`'s assumption that release-plz
uses its default `<package>-v<version>` tag format goes unchecked.

**Why it is worth closing despite a bounded blast radius.** release-plz itself rejects both
shapes, so such a config fails loudly at the release step rather than silently cutting wrong tags.
What makes it worth fixing anyway is that the failure mode is precisely the one SMA-603 was built
around: an inconclusive decision must BUILD, never skip. A guard that cannot distinguish "the
config is fine" from "I could not read the config" is the shape that issue exists to prevent, and
this one currently cannot.

## 2. Evidence

Read off the tree at `b5e0667` (`origin/main`). Nothing here needed a run to establish; each item
is a property of the code as written, and §7 lists what must nonetheless be MEASURED during
implementation rather than argued.

**E1 — the two bypasses are real, and they differ.** `workspace = []` reaches the membership test
with `{}` substituted (falsy `or`). `package = { name = "a" }` reaches the loop as a dict and
yields the string `"name"`, which the `isinstance` guard discards. The first bypass is a *falsy
substitution*; the second is a *type-blind iteration*. A single fix addressing only one leaves the
other.

**E2 — the same blind spot exists a second time, in `releasable_packages`.** Ten lines below the
call, the entry map re-implements the identical filter:

```python
entries = {p["name"]: p for p in (cfg.get("package") or [])
           if isinstance(p, dict) and isinstance(p.get("name"), str)}
```

With `package = { ... }` this yields an empty `entries`, so every crate reads as releasable and
its missing tag makes the decision BUILD. That direction is fail-safe, so E2 is not a second
defect — but it is the same pattern, duplicated, and a future edit to one copy cannot be assumed
to reach the other.

**E3 — the fix falsifies a measurement this file cites in five places.** Three docstrings justify
a deliberately-broad `except Exception` by citing `workspace = 3` raising a bare `TypeError` out
of `assert_default_tag_format`'s membership test: `run()` (`release_plan.py:212`), `_assert_repo()`
(`:478-479`), and `_malformed_config_asserts_three()` (`:409`). `README.md` repeats it at `:141-142`.
After this change that shape raises `InconclusiveError`. The broad catches must **stay** — they are
the floor for shapes nobody has thought of — but their stated rationale becomes a false claim, and
this repo treats a stale measurement as a defect.

**E4 — the collection-layer rows have no arity floor.** `self_test()` floors `FIXTURES` at 8 rows
and says why ("a self-test that silently stops testing anything still reads as a pass"). The six
collection-layer helpers below it are an inline tuple with no equivalent. Deleting a helper from
that tuple reds nothing: check 11's `--fixture-count` floor in `ci/actionlint/run.sh:4583` counts
`FIXTURES` only.

**E5 — the collection-layer loop is deletable in silence.** `ci/release-plan/run.sh`'s row 7 states
the reasoning for the `FIXTURES` loop verbatim — "with it gone, `--self-test` still returns 0 and
every other control here still passes, so the fixture table would guard nothing" — and closes it
with a mutation row. The collection-layer loop has the identical property and no such row.

**E6 — the real config is unaffected.** `rs/release-plz.toml` declares `[workspace]` as a table and
thirteen `[[package]]` entries as an array of tables. Both validations pass it unchanged, which is
AC4 by construction (still MEASURED per §7 M4 rather than asserted here).

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

`cfg.get(key, default)` replaces `cfg.get(key) or default` throughout. That substitution is the
fix's core: TOML has no null, so a present key always carries a non-None value, and the explicit
default is what routes `workspace = []` to the `isinstance` check instead of substituting `{}`
past it.

`assert_default_tag_format` then takes the already-validated sections rather than the raw config,
and drops its own guards:

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
entries = {p["name"]: p for p in package_entries if isinstance(p.get("name"), str)}
```

Validation happens exactly once, and E2's duplicated `isinstance(p, dict)` filter disappears
because the elements are already known to be tables.

**Deliberately out of scope: a nameless `[[package]]` entry.** The surviving
`isinstance(p.get("name"), str)` filter still drops an entry with no string `name`. That is
fail-safe today — an entry that cannot be matched to a crate leaves that crate reading as
releasable, so its missing tag BUILDS — and it is outside the AC. Named here so a reader does not
mistake it for an oversight.

### 3.2 Message markers are distinct, because the fixtures match on them

Each raise carries a marker no other raise in this module produces:

| Shape | Marker |
|---|---|
| non-table `workspace` | `[workspace] is not a table` |
| non-array `package` | `[[package]] is not an array of tables` |
| non-table element | `[[package]] entry <i> is not a table` |

Distinctness is load-bearing. `_tag_name_override_is_inconclusive`'s docstring records the MEASURED
anti-pattern: with a bare `except InconclusiveError: return None`, neutering the function under
test kept that helper passing, because an unrelated `InconclusiveError` from further down the call
chain satisfied it. Every new fixture matches its own marker and rejects any other cause with a
"wrong reason" report.

### 3.3 Fail-safe direction is unchanged

Every new raise is `InconclusiveError`. `run()` converts it to `nothing_to_release=false` (BUILD);
`_assert_repo()` converts it to exit 3. Nothing introduced here can produce a skip. This is the one
invariant in `ci/release-plan/` that no change may weaken.

### 3.4 Three new collection-layer fixtures

Bringing the tuple to nine rows:

* `_workspace_not_a_table_is_inconclusive` — `workspace = []`, the falsy shape the issue names.
  (`workspace = 3`, the truthy-but-wrong shape, is already covered by
  `_malformed_config_asserts_three`, which after this change exercises the same validator.)
* `_package_not_an_array_of_tables_is_inconclusive` — `package = { name = "a" }`.
* `_package_entry_not_a_table_is_inconclusive` — `package = ["a"]`.

**The third is a deviation from the approved design, stated rather than slipped in.** That design
named two fixtures. AC2's validator has two distinct branches — the section is not a list, and an
element is not a table — and leaving the second branch unexercised would reproduce this issue's own
theme one level down. Each fixture's tree deliberately carries **no** `rs/crates/` directory, so a
neutered validator falls through to `crate_manifests`' unrelated "no crate manifests" error and the
marker match reports a wrong reason instead of a pass.

### 3.5 An arity floor on the collection-layer rows (E4)

The inline tuple moves to a module-level constant so it can be counted, and `self_test()` gains a
floor mirroring the `FIXTURES` one. Floored at **8** against nine actual rows: one row of headroom,
the "floor, not a count" idiom check 11 states, so a legitimate row removal does not abort the gate.
`FIXTURES` is untouched, so check 11's own `--fixture-count >= 8` is unaffected.

### 3.6 Row 8 of the negative control (E5)

`ci/release-plan/run.sh` gains a row mirroring row 7's construction: it `sed`s a COPY of
`release_plan.py` to neuter the `workspace` shape check, asserts via `cmp -s` that the mutation
actually changed the file (so a refactor cannot make the row vacuous), and asserts the mutant's
`--self-test` exits 3. Deleting the collection-layer loop, deleting the new fixtures, or neutering
the validator all make the mutant pass and this row red.

The row's guard uses a local named `mut8_rc`, distinct from row 7's `mut_rc`. That rename is not
cosmetic: `ci/affected-graph/ci_targets.py`'s comment on `RUFF_SH_CALL_SITES` records the measured
failure where a pinned guard line was satisfied by an unrelated identical line elsewhere in the
same file — "a pin that looks present and asserts nothing, one level down from the bug it exists to
close" — and cites `release-plan`'s own `mut_rc` as the precedent for avoiding it.

### 3.7 Registry obligations

`RELEASE_PLAN_SH_CALL_SITES` in `ci/affected-graph/ci_targets.py` pins whole stripped lines inside
`run.sh` and gains a tenth entry, row 8's assertion line `if [ "$mut8_rc" != "3" ]; then` — the
treatment entries 7 and 8 already receive, for the reason `WORKFLOW_CREDENTIALS_SH_CALL_SITES`
measured (deleting every assertion left four structural pins byte-identical and the control exited
0 having asserted nothing). Two prose sites count that tuple and must move with it: the enumeration
in `ci_targets.py`'s own comment (items 1-9) and `moon.yml:216-217` ("pins nine load-bearing
lines"). Implementation must check for a numeric assertion on the tuple's length before assuming
prose is the only consumer.

Reachability is already in place and needs no change: `moon.yml` lists `ci/release-plan/**/*` among
`repo:affected-smoke`'s inputs and `ci/actionlint/run.sh`'s `T_AFFECTED_SMOKE_REQUIRED_INPUTS`
floors that entry.

### 3.8 Documentation corrections (E3)

The three docstrings and the two `README.md` sites keep their broad-catch rationale but stop citing
a shape that is now typed: the catch exists for the **residual** — shapes this validator does not
model — and `workspace = 3` is named as the historical motivation, explicitly marked as no longer
producing a `TypeError`. `README.md`'s "six collection-layer rows" becomes nine and enumerates the
new ones; its `--negative-control` "seven rows" becomes eight and describes row 8.

## 4. What this does NOT do

* It does not parse a custom `git_tag_name` template. The checker still refuses and builds.
* It does not model `[workspace] exclude`, which `workspace_members` still refuses outright.
* It does not make a nameless `[[package]]` entry an error (§3.1).
* It does not narrow either broad `except Exception`. Both stay exactly as they are.

## 5. Acceptance criteria mapping

| AC | Where |
|---|---|
| 1. non-table `workspace` raises `InconclusiveError` | §3.1, fixture `_workspace_not_a_table_is_inconclusive` |
| 2. non-array-of-tables `package` raises `InconclusiveError` | §3.1, fixtures `_package_not_an_array_of_tables_is_inconclusive` and `_package_entry_not_a_table_is_inconclusive` |
| 3. both have `--self-test` fixtures, proven to red by mutation | §3.4 (fixtures), §3.6 (continuous mutation proof in CI), §7 M1-M3 (the one-time measurements) |
| 4. the real `rs/release-plz.toml` still passes unchanged | E6, verified by §7 M4 |

## 6. Risks

**R1 — a fixture that passes for the wrong reason.** Mitigated by §3.2's distinct markers and by
each fixture's crate-less tree, which guarantees a *different* error surfaces when the validator is
neutered. M1-M3 are what prove it rather than assume it.

**R2 — row 8's `sed` target drifts.** If the mutated line is reworded, the `sed` matches nothing and
the row would go vacuous. Mitigated by the `cmp -s` vacuity assertion, which reds instead.

**R3 — `RELEASE_PLAN_SH_CALL_SITES`' new entry is not unique in `run.sh`.** Mitigated by the
`mut8_rc` rename (§3.6); implementation must verify the line occurs exactly once, as every existing
entry was.

## 7. Measurements required during implementation

Each must be run and its result recorded; none may be argued from the code.

* **M1** — neuter the `workspace` shape check; confirm `_workspace_not_a_table_is_inconclusive`
  reports a *wrong reason* (not a pass) and `--self-test` exits 3.
* **M2** — neuter the `package` section check; same, for
  `_package_not_an_array_of_tables_is_inconclusive`.
* **M3** — neuter the element check; same, for `_package_entry_not_a_table_is_inconclusive`.
* **M4** — `bash ci/release-plan/run.sh --assert` against the real repository exits 0 (AC4).
* **M5** — `--self-test` and `--negative-control` both pass, row 8 included.
* **M6** — `repo:actionlint` (check 11) and `repo:affected-smoke` (the `RELEASE_PLAN_SH_CALL_SITES`
  pin) both green.
