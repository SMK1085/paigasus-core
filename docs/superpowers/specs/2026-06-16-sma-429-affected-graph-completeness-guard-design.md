# SMA-429 — Affected-graph guard: strict-equality (default-deny) meta-check replacing the growing forbid enumeration

**Status:** approved design (brainstorm complete, ready for plan)
**Linear:** [SMA-429](https://linear.app/smaschek/issue/SMA-429/affected-graph-guard-add-a-completenessdefault-deny-meta-check-replace)
**Date:** 2026-06-16
**Follow-up of:** [SMA-420](https://linear.app/smaschek/issue/SMA-420) review finding **F4** (raised by both staff review and CodeRabbit).
**Builds on:** [SMA-409](https://linear.app/smaschek/issue/SMA-409) (stood up `ci/affected-graph/run.sh` with the positive-superset + explicit-negative model), SMA-419/420 (landed the py and ts kernel-wrapper edges that grew the forbid enumeration).
**Reviewed by:** staff-engineer design review (2026-06-16); dispositions in the final section.

## Goal

Replace the `kernel->bindings` case's hand-maintained **forbid-regex enumeration of every
unrelated package** with a meta-check that needs no per-package maintenance. Each newly-added
ts/py project currently has to be hand-added to that ~70-char regex or it is **silently
unasserted** — a false sense of security that compounds with every new project.

**Secondary, coupled deliverable:** bump the pinned moon from 2.2.5 to 2.3.2 (§1). Strict
equality couples the guard to a specific moon version's affected-set output, so the guard must
be grounded on — and ship in the same change as — the version CI actually runs. The repo's pin
currently lags reality (`.prototools` = 2.2.5 while CLAUDE.md and the `.moon/` config already
assume 2.3.2), so this issue pins 2.3.2 and grounds the guard there in one diff.

## The decision this issue owns

A completeness/default-deny check **reverses** SMA-409's deliberate model:
*"positive-superset + explicit-negative, not strict equality, to stay robust as projects are
added."* SMA-409 feared that strict equality would be brittle as the monorepo grows. This
issue owns that trade-off and resolves it in favour of **strict equality (default-deny)** —
see the decision below.

## Decision resolved during brainstorming

**Strict equality / default-deny: assert `affected_set == expected_set` per case; drop the
`FORBID_REGEX` concept entirely.** The expected set becomes the complete allowlist — any
unlisted project that appears fails the case. Chosen over a "completeness check + broad
category forbid" alternative (which would keep an include/exclude split and a forbid regex)
because it is simpler and strictly safer here. Grounding is on **moon 2.3.2** — the version this
issue pins (§1) and the version the guard ships with. The affected sets are byte-identical on
both 2.2.5 and 2.3.2, so the bump is affected-graph-neutral. Each case's `moon query projects
--affected --downstream deep` set (minus `repo`) is **exactly** its current must-include set
today — zero extras across all four cases, and `repo` is the only universal-appearer needing a
filter:

| case | synthetic touch | affected set (minus `repo`) |
| --- | --- | --- |
| `contracts->proto` | `contracts/proto/paigasus/gateway/v1/health.proto` | `contracts`, `paigasus-proto-{rs,py,ts}`, `paigasus-gateway-rs` |
| `kernel->bindings` | `rs/crates/libs/paigasus-kernel/src/lib.rs` | `paigasus-kernel-rs`, `paigasus-py-bindings-rs`, `paigasus-gateway-rs`, `paigasus-kernel-py`, `paigasus-node-bindings-rs`, `paigasus-kernel-ts` |
| `binding-oneway` | `rs/crates/bindings/paigasus-py-bindings/src/lib.rs` | `paigasus-py-bindings-rs`, `paigasus-kernel-py` |
| `binding-oneway-node` | `rs/crates/bindings/paigasus-node-bindings/src/lib.rs` | `paigasus-node-bindings-rs`, `paigasus-kernel-ts` |

So strict equality is exact today, and all four forbid-regexes are already redundant — they
exist only to *assert* isolation, which strict equality does for free.

**Why the brittleness SMA-409 feared is actually the desired behaviour:**

- A new **unrelated** project never enters a case's downstream set → it never appears → **zero
  maintenance** (the regex required hand-editing for exactly these).
- A project that **genuinely** joins a case's downstream set (e.g. the deferred wasm kernel
  binding) makes the case fail red → a **forced, conscious one-line update** to the expected
  set — which is precisely when you want a human to confirm the new edge is intended.
- It converts today's failure mode from **"silently unasserted"** (forget to extend the regex →
  a real cross-stack leak slips through green) into **"loudly fails"** (any unlisted project,
  whether leak or new dependent, fails the case).

## 1. Toolchain — pin moon 2.3.2 (`.prototools` bump)

`.prototools` currently pins `moon = "2.2.5"`, but the repo already behaves as a 2.3.2
workspace: CLAUDE.md documents "Moon is 2.3.2" and the `.moon/` config uses 2.3.2-era keys
(`vcs.client`, `codeowners.sync`); CI stays green only because 2.2.5 tolerates them. This issue
bumps the pin so it matches the docs and the config.

- **Change:** `.prototools` `moon = "2.2.5"` → `"2.3.2"`. No CLAUDE.md edit is needed — its
  "Moon is 2.3.2" gotchas become *true* once the pin lands.
- **Why fold it in (not a separate chore):** strict equality couples the guard to moon's
  affected-set output, so the guard must be grounded on the exact version it ships with. Pinning
  2.3.2 and grounding the guard on 2.3.2 in one diff keeps them honest.
- **Affected-graph-neutral:** the four affected sets are byte-identical on 2.2.5 and 2.3.2
  (verified), so the bump changes no expected set. moon 2.3.2 already parses this workspace's
  full `.moon/` + per-project `moon.yml` config cleanly (a 2.3.2 `moon query projects` against
  the repo returned the full project list without error).
- **Blast radius:** this is a repo-wide toolchain change, so verification includes a **full
  `moon ci` green on 2.3.2**, not just the affected-graph guard (Verification §3).

## 2. Core mechanism — `assert_case` becomes strict-equality

Drop the 4th `FORBID_REGEX` parameter. New signature: `assert_case LABEL FILE EXPECTED_CSV`.

```
got  = affected_ids(file)        # already sorted, newline list, minus `repo`
want = sorted(split EXPECTED_CSV on ',')
PASS iff got == want
```

- `repo` stays filtered (its source is `.`, so it owns every file and appears for every touch —
  pure noise). No other aggregate needs filtering: the data shows the `py` / `ts` / `contracts`
  *root* projects never appear in the rust-side cases, and `contracts` legitimately appears (and
  is expected) in the contracts case.
- The 3-way return code is **unchanged**: 0 pass / 1 assertion-fail / 2 infrastructure-error.
  A dead `moon query` still returns 2 and aborts the whole guard (exit 2) so a broken `moon` is
  never mistaken for a graph regression.
- The `grep -E -- "$forbid"` leak check and its `--` option-guard comment are deleted with the
  forbid concept.

## 3. Failure messaging

On mismatch, split the diff into two buckets with actionable guidance — because strict equality
*will* fail the day a legitimate new dependent lands, and the message must tell the maintainer
what to do:

```
FAIL  [kernel->bindings] affected set != expected set
  missing  (expected but absent — likely a dropped dependsOn edge or a lost --include-relations):
    paigasus-kernel-ts
  unexpected (present but not expected — a cross-stack leak/regression, OR a legitimately new
  dependent: if the new edge is intended, add it to this case's expected set):
    paigasus-sdk-ts
```

Computed with `comm` over the two already-sorted lists: `comm -23` = missing (in `want`, not
`got`), `comm -13` = unexpected (in `got`, not `want`). Both buckets are reported in one failure
so a maintainer sees the full picture at once.

## 4. The four cases + negative controls

- **All four cases keep today's must-include set verbatim** as their new `EXPECTED_CSV`; the
  forbid arguments are deleted. The `kernel->bindings` case loses the entire ~70-char regex.
- The one-directionality intent of the two `binding-oneway*` cases (a binding edit must **not**
  drag in `paigasus-kernel-rs`) is now enforced **implicitly** — `paigasus-kernel-rs` simply
  isn't in the expected set — and preserved as a **code comment**, not a regex.
- **Existing `neg-wrong-expect` control stays** (a wrong expectation → red), with its empty
  forbid argument dropped to match the new signature.
- **Add a second negative control proving the default-deny direction:** feed a kernel edit a
  deliberately **incomplete** expected set (e.g. just `paigasus-kernel-rs`, omitting the other
  five) and assert the harness reports red on the *unexpected* extras. This directly validates
  the new guarantee this issue exists to add — that an under-specified expected set fails red —
  which the old positive-superset model could not catch. Cheap, high-signal.

## 5. README + comments

- Rewrite README's *"Maintenance — the must-exclude assertions are topology-coupled (SMA-409
  F5)"* section: there is no longer an include/exclude split or a hand-maintained enumeration.
  New content: expected sets are **exact** (default-deny); cross-stack isolation is enforced
  implicitly; when a project legitimately becomes a new dependent the test fails and names the
  exact one-line edit to make (add it to that case's expected set after confirming the edge is
  intended). Also state that each expected set is a **snapshot of `moon query --affected
  --downstream deep` output at the pinned moon version** — a moon upgrade that changes the
  affected-set output, even benignly, will now fail the guard, so re-grounding the expected sets
  is a known step of any moon bump (the version coupling is the deliberate flip side of strict
  equality, F2).
- Trim the now-redundant *"still no unrelated `*-py` / `*-ts`"* parentheticals from the per-case
  bullets — that isolation is implicit now.
- Update `run.sh`'s header / `assert_case` doc comments and the inline case comments that
  reference the forbid-regex, the positive-superset model, and the `--` grep guard (all removed).

## Verification (maps to acceptance criteria)

1. **Forbid enumeration removed.** No `FORBID_REGEX` parameter, no per-package regex, and no
   `grep -E -- "$forbid"` remain in `run.sh`.
2. **Completeness/default-deny in place.** Each case asserts `affected == expected`; any project
   appearing that is not in the expected set fails the case (proven by the new incomplete-expected
   negative control).
3. **Pin bumped + CI green on 2.3.2.** `.prototools` pins `moon = "2.3.2"`; `proto install` +
   `moon setup` provision it, and a **full `moon ci` is green on 2.3.2** (the repo-wide blast
   radius of the bump, not just the guard). `ci/affected-graph/run.sh` run under the repo-context
   moon (now 2.3.2) → green across all four cases (each expected set equals the affected set
   exactly).
4. **Guard still fails red on regressions.** `ci/affected-graph/run.sh --negative-control` →
   red on **both** the wrong-expectation control and the new incomplete-expectation control.
5. **No harness/CI surface change.** `moon run repo:affected-smoke` and the `ci.yml` task array
   are unchanged; the `assert_include_relations` check (SMA-409 F1) is untouched.
6. **Query-depth ↔ build-depth equivalence (F3, inherited from SMA-409).** The guard's
   `--downstream deep` query asserts 2-hop cascades (kernel → binding → wrapper). Confirm a real
   `moon ci --include-relations` on 2.3.2 actually rebuilds the 2-hop wrappers
   (`paigasus-kernel-py` / `-ts`), so the guard never passes against a set CI doesn't rebuild.
   The query itself is **unchanged** by this issue, so this is a pre-existing property (already
   exercised when SMA-419/420 added those wrappers to a green CI); a full scratch-branch
   integration test stays out of scope — SMA-409 deliberately rejected it.

## Out of scope

- The `assert_include_relations` check and the guard's `inputs` / `ci.yml` integration (SMA-409)
  — unchanged.
- Adding new guard *cases* for projects that don't yet wrap the kernel (e.g. a wasm binding) —
  those land with their respective binding issues; this issue only changes the classification
  model of the existing four cases.
- Any change to the moon graph, edges, or `--include-relations` wiring.
- Upgrading moon *beyond* 2.3.2, or auditing 2.3.2 for behavior changes outside the affected
  graph — the bump targets exactly 2.3.2 (the version the repo already assumes); the verification
  gate is a green `moon ci` on 2.3.2 (§3), not a changelog audit.

## Review dispositions (staff review, 2026-06-16)

- **F1 (Medium) — strict-equality grounding used moon 2.3.2, but `.prototools` pins 2.2.5.**
  Accepted; the original grounding ran on the *global* `~/.proto/bin/moon` (2.3.2) rather than the
  repo-context shim (2.2.5). Root cause was a real repo drift — `.prototools` lagged at 2.2.5
  while CLAUDE.md and the `.moon/` config already assumed 2.3.2. **Resolution (owner decision):**
  fold the pin bump into this issue — `.prototools` → `moon = "2.3.2"` (§1) — and ground the guard
  on 2.3.2, the version it ships with. The affected sets are byte-identical on 2.2.5 and 2.3.2, so
  the bump is affected-graph-neutral; the verification gate widens to a full `moon ci` green on
  2.3.2 (§3) to cover the repo-wide blast radius.
- **F2 (Low) — make the moon-version coupling explicit in the README.** Accepted; the README
  rewrite (§5) now states each expected set is a snapshot of `moon query --affected --downstream
  deep` output at the pinned moon version, making re-grounding a known step of any moon bump.
- **F3 (Low) — confirm `--downstream deep` matches CI's `--include-relations` resolution.**
  Accepted as a verification line item (§6), not a design change: the query is inherited unchanged
  from SMA-409 and the 2-hop cascade was already exercised when SMA-419/420 added the py/ts
  wrappers to a green CI. A full scratch-branch integration test remains out of scope (per
  SMA-409's deliberate rejection); the confirm folds into the 2.3.2 `moon ci` verification run.
