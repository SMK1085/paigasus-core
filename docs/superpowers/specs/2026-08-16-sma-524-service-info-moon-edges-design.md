# SMA-524 — `paigasus-service-info` Moon graph edges

Linear: [SMA-524](https://linear.app/smaschek/issue/SMA-524/rust-paigasus-service-info-is-missing-its-moon-edge-to-paigasus-proto)
Related: [SMA-505](https://linear.app/smaschek/issue/SMA-505) (added the crate; owns the affected-graph
expected set this edits), [SMA-438](https://linear.app/smaschek/issue/SMA-438) (found the gap; left the
note this deletes), [SMA-389](https://linear.app/smaschek/issue/SMA-389) (D3 — the `^:build` rule),
[SMA-409](https://linear.app/smaschek/issue/SMA-409) / [SMA-429](https://linear.app/smaschek/issue/SMA-429)
(the strict-equality guard being extended; F3 of SMA-429 is the gap § 3 closes)

Revised after an adversarial challenge; § 9 records what the challenge changed.

## 1. Context

### 1.1 The two-part invariant

Cargo path deps are **not** auto-synced into Moon's project graph in this repo — every in-tree edge is
hand-declared in a `moon.yml`. Two separate declarations are required, and **they do different jobs**:

| Declaration | What it does |
| --- | --- |
| project-level `dependsOn: ['<id>']` | creates the **project-graph** edge |
| task-level `deps: ['^:build']` on `build`/`test` | makes the **task graph** schedule the upstream's build ahead of this project's `build`/`test` |

§ 2.4 proves these are independent: each can be present without the other, and only both together
produce a correctly-built CI.

> **Correction to the received wording.** SMA-389 D3 — quoted verbatim in
> `rs/crates/libs/paigasus-kernel-parity/moon.yml:7-10` and `rs/crates/libs/paigasus-proto/moon.yml:7-10`,
> and repeated in the SMA-524 issue text — says the task-level `^:build` "is what propagates
> `affected`". Measured on moon 2.3.2 (§ 2.4) that is true of the **task** graph and false of the
> **project** graph: `moon query projects --affected` follows `dependsOn` alone and is completely blind
> to `^:build`. The distinction is the whole subject of § 3, so this spec states it precisely rather
> than inheriting the loose phrasing.

A hand-declared edge is a hand-forgettable edge. Forgetting one does not go red: it silently
under-builds and stays green.

### 1.2 Measurement provenance

Every number below was produced in a clean worktree at `origin/main` (`4546c6a`) with the
repo-pinned moon resolved through the proto shim — **not** the global binary. SMA-429's own review
finding F1 was that these sets had been grounded on the wrong moon, and strict equality is
version-coupled by design, so this is recorded rather than assumed:

```
$ which moon
/Users/smaschek/.proto/shims/moon
$ moon --version
moon 2.3.2
```

Reproduce any row with:

```bash
printf '%s\n' <touched-file> \
  | moon query projects --affected --downstream deep \
  | python3 -c 'import sys,json; print(", ".join(sorted(p["id"] for p in json.load(sys.stdin)["projects"] if p["id"]!="repo")))'
```

## 2. Findings

### 2.1 The audit, in both directions

Every crate's Cargo in-tree deps were diffed against its `moon.yml` `dependsOn` and task `^:build`,
**in both directions**. Across all 13 crates:

| # | Consumer | Drift | Direction | Severity |
| --- | --- | --- | --- | --- |
| 1 | `paigasus-service-info-rs` | missing `dependsOn: paigasus-proto-rs` | Cargo → Moon | under-builds |
| 2 | `paigasus-service-info-rs` | missing `^:build` on `build`, `test` | Cargo → Moon | under-builds |
| 3 | `paigasus-iam-rs` | missing `dependsOn: paigasus-service-info-rs` | Cargo → Moon | under-builds |
| 4 | `paigasus-gateway-rs` | missing `dependsOn: paigasus-service-info-rs` | Cargo → Moon | under-builds |
| 5 | `paigasus-gateway-rs` | **declares** `dependsOn: paigasus-kernel-rs` with no Cargo dep | Moon → Cargo | over-builds |

Evidence: `rs/crates/libs/paigasus-service-info/Cargo.toml:13`,
`rs/crates/services/paigasus-iam/Cargo.toml:121`,
`rs/crates/services/paigasus-gateway/Cargo.toml:73`. For #5,
`rs/crates/services/paigasus-gateway/moon.yml:9` declares the edge while `grep -rn kernel
rs/crates/services/paigasus-gateway/` returns **only that one line** — no Cargo dep, no source
reference.

**The two directions are not equally dangerous.** #1–#4 under-build: CI runs less than it should and
stays green — a silent correctness hole. #5 over-builds: CI runs more than it needs — a cost, never a
correctness risk. This asymmetry drives D3 and D6.

### 2.2 Two holes, only one of which the issue describes

```
$ printf '%s\n' rs/crates/libs/paigasus-proto-derive/src/lib.rs | moon query projects --affected --downstream deep | …
paigasus-gateway-rs, paigasus-iam-rs, paigasus-proto-derive-rs, paigasus-proto-rs

$ printf '%s\n' rs/crates/libs/paigasus-service-info/src/lib.rs | moon query projects --affected --downstream deep | …
paigasus-service-info-rs
```

- **Hole A** (the issue): a proto edit does not reach `paigasus-service-info-rs`.
- **Hole B** (undocumented, guarded by **no** existing case): a `service-info` edit reaches nothing but
  itself. `paigasus-service-info` is currently a graph **leaf**.

Hole B is the more severe. An edit to `ServiceInfoDto` — the wire body **both** services return —
rebuilds and retests nothing downstream, so a breaking serialization change ships green.

### 2.3 Fixing all edges perturbs exactly one existing expected set

| Case | Before | After |
| --- | --- | --- |
| `contracts->proto` | 7 projects | **unchanged** |
| `proto-derive->proto` | 4 projects | **+ `paigasus-service-info-rs`** |
| `kernel->bindings` | 10 projects | **unchanged** |

`contracts->proto` is unchanged because `paigasus-service-info-rs` already appeared there through its
own `contracts:generate` task dep — never through an edge to `paigasus-proto`. That coincidence is
exactly what masked hole A. `kernel->bindings` is unchanged because `iam`/`gateway` are graph leaves:
giving a leaf a new *upstream* cannot change any `--downstream deep` result.

### 2.4 The guard cannot see half the invariant — SMA-429 F3, measured

This is the finding that reshapes the design. Three variants of
`paigasus-service-info/moon.yml`, each queried both ways:

| Variant | `projects --affected` (what the guard asserts) | `tasks --affected` (what CI runs) |
| --- | --- | --- |
| `dependsOn` + `^:build` (the fix) | service-info **present** | `service-info-rs:build`,`:test` **present** |
| `dependsOn` only, no `^:build` | service-info **present** | `service-info-rs:build`,`:test` **ABSENT** |
| `^:build` only, no `dependsOn` | service-info **absent** | — |

**Row 2 is the problem.** Delete the `^:build` this spec adds and *every* affected-graph case stays
green while `moon ci --include-relations` under-builds exactly as it does today. The project-level
guard is blind to `^:build` by construction — it walks the project graph, and `^:build` creates no
project edge; it *consumes* one.

So wiring the edges and adding only a project-level case would leave half of the fix protected by
nothing. This is the "query-depth ↔ build-depth equivalence" gap SMA-429 logged as F3
(`docs/superpowers/specs/2026-06-16-sma-429-affected-graph-completeness-guard-design.md:171-177`) and
deferred. This issue is where it bites, so this issue closes it.

### 2.5 `lint` and `fmt` do not propagate across any edge (pre-existing)

`.moon/tasks/rust.yml:25-30` gives `lint` (`cargo clippy --all-targets -- -D warnings`) and `fmt` no
`deps`, and no project overrides them. Measured — a `paigasus-proto` edit schedules:

```
paigasus-proto-rs:build  paigasus-proto-rs:build-release  paigasus-proto-rs:fmt  paigasus-proto-rs:lint
paigasus-proto-rs:test   paigasus-gateway-rs:build  paigasus-gateway-rs:test
paigasus-iam-rs:build    paigasus-iam-rs:test       repo:machete
```

The upstream gets `lint`; the downstream consumers get **only** `build` and `test`. A change that trips
`-D warnings` in a consumer therefore still ships green and reds `main` after merge.

This is **repo-wide and pre-existing** — it affects all 13 crates, not just these edges. Recorded here
because § 1.1 will be quoted by future issues the way this spec quotes SMA-389 D3, and an incomplete
statement of the invariant propagates. Not fixed here (D7).

### 2.6 This PR's own CI exercises only the guard

The PR touches `moon.yml` files, `run.sh`, and this spec. Measured, that set affects exactly one task:

```
repo:affected-smoke
```

No Rust `build`/`test` task is scheduled, because `.moon/tasks/rust.yml:18` gives `build` the inputs
`['@group(sources)', 'Cargo.toml']` — a project's own `moon.yml` is not among them. `repo:affected-smoke`
*does* list `rs/crates/*/*/moon.yml` and `rs/**/Cargo.toml` in its `inputs` (`moon.yml:117-136`), which is
why it runs.

**Consequence for verification:** a full `moon ci` run on this PR proves nothing about the new edges.
The guard is the only thing that tests them, which is precisely why § 3 must strengthen the guard
rather than lean on the gate graph.

## 3. The design change this forces

Adding a project-level case alone would assert `dependsOn` and silently ignore `^:build` (§ 2.4). The
guard therefore gains a **task-level** assertion class.

`assert_task_case LABEL FILE EXPECTED_CSV` runs `moon query tasks --affected --downstream deep` and
compares the resulting set of **`build` and `test`** tasks, by strict equality, against an expected set.

Scoping to `build`/`test` is deliberate: those are the two tasks that carry `^:build`, so they are
exactly the invariant under assertion. Including `fmt`/`lint`/`build-release`/`repo:machete` would
couple every case to unrelated task config (§ 2.5) and make the guard brittle without adding assurance
about `^:build`.

Measured expected sets, and the control proving the assertion bites:

| Touched file | `build`/`test` tasks scheduled |
| --- | --- |
| `paigasus-proto/src/lib.rs` (with fix) | gateway `build`,`test`; iam `build`,`test`; proto `build`,`test`; **service-info `build`,`test`** |
| `paigasus-service-info/src/lib.rs` (with fix) | gateway `build`,`test`; iam `build`,`test`; service-info `build`,`test` |
| `paigasus-proto/src/lib.rs`, `^:build` removed | **service-info `build`,`test` drop out** → case reds |

## 4. Decisions

**D1 — Fix all four under-building edges, not just the one the issue names.** Same defect, same root
cause, same crate, found in the same audit. Shipping hole A alone leaves the crate half-wired and
leaves the more severe hole open.

**D2 — Add a project-level `service-info->services` case.** Hole B is asserted by no existing case.
Its expected set omits `paigasus-proto-rs`, so the case also catches an edge *reversal*. Note the
precise limit: because the query is downstream-only, `paigasus-proto-rs` can enter this set only if
the edge is reversed; *adding* the reverse edge alongside the forward one is a **cycle**, which Moon
rejects at graph construction and which surfaces as `run_case` rc=2 → `exit 2` ("infrastructure
error"), not a clean assertion failure. D2 claims no more than that.

**D3 — Add task-level cases (§ 3).** Without them, half of D1 ships unguarded (§ 2.4). This closes
SMA-429 F3 for these edges.

**D4 — Retain the phantom `gateway → kernel` edge; do not remove it here.** It over-builds, which is
the safe direction (§ 2.1), so it is not a silent hole. Removing it would drop `paigasus-gateway-rs`
from the `kernel->bindings` expected set — an unrelated change to a guard case this issue declares out
of scope, in service of a cost optimisation nobody asked for. Instead: record it in
`paigasus-gateway/moon.yml` as a known over-approximation and file a follow-up. **Not doing this
silently is the point** — an undocumented phantom edge is what made § 2.1's first pass wrong.

**D5 — Defer the generic Cargo↔Moon parity check to a follow-up; add a compensating control now.**
The honest rationale, replacing an earlier and incorrect cost argument: the infrastructure does
*already* exist — `repo:affected-smoke` declares `rs/**/Cargo.toml` and `rs/crates/*/*/moon.yml` in its
`inputs`, and `run.sh` already ships a `--negative-control` harness — so this is a design question, not
a plumbing one. What it genuinely needs is a policy for the Moon→Cargo direction: #5 in § 2.1 must be
an *allowed* over-approximation, so the check needs an allowlist, and an allowlist needs a rule for
what may go in it. That is its own issue.

The real weakness this exposes: the strict-equality guard only asserts edges someone remembered to
write a **case** for. SMA-505 added a crate with no case, which is why this bug survived a full review
cycle. Adding a case for `paigasus-service-info` fixes today's crate and does nothing for the next one.
Compensating control shipped here: a `CLAUDE.md` gotcha stating that a new in-tree Cargo dep needs
`dependsOn` **+** task `^:build` **+** its own guard case.

**D6 — Mirror `paigasus-proto/moon.yml` exactly.** `deps: ['contracts:generate', '^:build']` is already
shipping on `paigasus-proto-rs` with `contracts` as an implicit parent that has **no** `build` task
(`contracts/moon.yml` defines only `generate`/`lint`/`fmt`/`breaking`). So Moon 2.3.2 silently skips
parents lacking the task — proven in this repo by the green baseline, not assumed.

**D7 — Do not fix the `lint`/`fmt` propagation hole (§ 2.5).** Repo-wide and pre-existing; it affects
all 13 crates. Fixing it here would change the task graph for every crate under cover of a
three-edge fix. Documented + follow-up.

## 5. Changes

1. **`rs/crates/libs/paigasus-service-info/moon.yml`** — gains `dependsOn: ['paigasus-proto-rs']` and
   `^:build` on `build`/`test`, with a comment stating the two-part invariant per § 1.1.
2. **`rs/crates/services/paigasus-iam/moon.yml`**, **`.../paigasus-gateway/moon.yml`** — each gains
   `paigasus-service-info-rs` in `dependsOn`. Both already carry `^:build`, so no task change. The
   gateway file also gains the D4 note on its kernel edge.
3. **`ci/affected-graph/run.sh`** —
   - `proto-derive->proto` expected set gains `paigasus-service-info-rs`;
   - SMA-438's *"`paigasus-service-info-rs` is deliberately ABSENT"* paragraph is deleted, **together
     with the bare `#` separator line that precedes it**;
   - new project case `service-info->services`, touched file
     `rs/crates/libs/paigasus-service-info/src/lib.rs` (matching all six existing cases);
   - new `assert_task_case` helper + two task cases (§ 3);
   - `contracts->proto` and `kernel->bindings` expected sets **unchanged** (§ 2.3).
4. **`ci/affected-graph/README.md`** — add the new cases **and** correct three pre-existing staleness
   bugs: the contracts bullet omits `paigasus-iam-rs` and `paigasus-service-info-rs` (both in the live
   set at `run.sh:108`), the kernel bullet omits `paigasus-iam-core-rs`/`paigasus-iam-rs`, and there is
   **no bullet at all** for `proto-derive->proto`. Nothing gates this file — noted in the PR.
5. **`CLAUDE.md`** — the D5 compensating-control gotcha.

## 6. Verification

1. `bash ci/affected-graph/run.sh` — strict equality re-verifies **every** case, so the unchanged ones
   are proven unchanged, not assumed.
2. `bash ci/affected-graph/run.sh --negative-control` — the harness still reds on wrong expectations.
3. **Bite checks** (each must fail, then be restored):
   - revert the three `dependsOn` edges → project cases red;
   - revert **only** the `^:build` → project cases stay green, **task cases red**. This is the check
     that proves § 2.4's hole is closed; without it the task cases could pass vacuously.
4. The full gate graph per `CLAUDE.md`. Note § 2.6: this proves the repo still builds, **not** that the
   new edges work — the guard is what tests those.

## 7. Cost accepted

After this change, any edit to `paigasus-service-info/src/lib.rs` schedules `paigasus-iam-rs:test` —
the Docker-backed testcontainers suite (Postgres/Redis/NATS) in a job with `timeout-minutes: 30` and a
documented disk-exhaustion history (`.github/workflows/ci.yml:22-29`). That is the correct trade: it is
the entire point of the fix. Recorded so it is not later rediscovered as a regression.

## 8. Rollback

`git revert` of the single commit. Recorded because strict equality means a wrong expected set reds
`main` for **every** contributor until reverted, and `repo:affected-smoke` is a required check.

## 9. Open questions and follow-ups

**Open — does Moon 2.3.2's Rust toolchain support inferring project edges from Cargo?** If it is an
unset opt-in, enabling it would subsume both D1's hand-wiring and D5 permanently. `.moon/toolchains.yml`
configures `rust` with `version`/`components`/`targets` only, and § 2.2 proves inference is **not**
happening today — but "not enabled" and "not supported" are different answers. Flipping it would be a
repo-wide architectural change against the convention SMA-389 established, so it is out of scope here;
worth a spike.

**Follow-ups to file:** (a) the generic bidirectional parity check + allowlist policy (D5); (b) the
phantom `gateway → kernel` edge (D4); (c) `lint`/`fmt` propagation (D7).

**What the adversarial challenge changed.** It found two blockers. § 2.1's audit was one-directional
and its "the only drift is …" conclusion was **false** — it missed the phantom `gateway → kernel` edge,
and D5's original rationale rested entirely on that wrong count. More consequentially, it showed the
proposed guard asserted only the `dependsOn` half of the invariant, so the `^:build` half would have
shipped protected by nothing — the same silent-hole class this issue exists to close. § 3, D3, D4, D5
and the § 6.3 bite checks are all downstream of that. It also caught the stale README, the
unrunnable § 2.2 repro, the missing measurement provenance, the `lint`/`fmt` gap, the unspecified
touched-file argument, and D2's over-claim about one-directionality.
