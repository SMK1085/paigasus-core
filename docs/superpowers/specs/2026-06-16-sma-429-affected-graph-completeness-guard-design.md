# SMA-429 — Affected-graph guard: strict-equality (default-deny) meta-check replacing the growing forbid enumeration

**Status:** approved design (brainstorm complete, ready for plan)
**Linear:** [SMA-429](https://linear.app/smaschek/issue/SMA-429/affected-graph-guard-add-a-completenessdefault-deny-meta-check-replace)
**Date:** 2026-06-16
**Follow-up of:** [SMA-420](https://linear.app/smaschek/issue/SMA-420) review finding **F4** (raised by both staff review and CodeRabbit).
**Builds on:** [SMA-409](https://linear.app/smaschek/issue/SMA-409) (stood up `ci/affected-graph/run.sh` with the positive-superset + explicit-negative model), SMA-419/420 (landed the py and ts kernel-wrapper edges that grew the forbid enumeration).

## Goal

Replace the `kernel->bindings` case's hand-maintained **forbid-regex enumeration of every
unrelated package** with a meta-check that needs no per-package maintenance. Each newly-added
ts/py project currently has to be hand-added to that ~70-char regex or it is **silently
unasserted** — a false sense of security that compounds with every new project.

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
because it is simpler and strictly safer here. Grounding from the live graph (moon 2.3.2),
each case's `moon query projects --affected --downstream deep` set (minus `repo`) is **exactly**
its current must-include set today — zero extras across all four cases:

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

## 1. Core mechanism — `assert_case` becomes strict-equality

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

## 2. Failure messaging

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

## 3. The four cases + negative controls

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

## 4. README + comments

- Rewrite README's *"Maintenance — the must-exclude assertions are topology-coupled (SMA-409
  F5)"* section: there is no longer an include/exclude split or a hand-maintained enumeration.
  New content: expected sets are **exact** (default-deny); cross-stack isolation is enforced
  implicitly; when a project legitimately becomes a new dependent the test fails and names the
  exact one-line edit to make (add it to that case's expected set after confirming the edge is
  intended).
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
3. **Guard still passes on the live graph.** `ci/affected-graph/run.sh` → green across all four
   cases (each expected set equals today's affected set exactly).
4. **Guard still fails red on regressions.** `ci/affected-graph/run.sh --negative-control` →
   red on **both** the wrong-expectation control and the new incomplete-expectation control.
5. **No harness/CI surface change.** `moon run repo:affected-smoke` and the `ci.yml` task array
   are unchanged; the `assert_include_relations` check (SMA-409 F1) is untouched.

## Out of scope

- The `assert_include_relations` check and the guard's `inputs` / `ci.yml` integration (SMA-409)
  — unchanged.
- Adding new guard *cases* for projects that don't yet wrap the kernel (e.g. a wasm binding) —
  those land with their respective binding issues; this issue only changes the classification
  model of the existing four cases.
- Any change to the moon graph, edges, or `--include-relations` wiring.
