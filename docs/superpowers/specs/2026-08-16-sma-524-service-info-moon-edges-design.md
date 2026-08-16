# SMA-524 — `paigasus-service-info` Moon graph edges + Cargo↔Moon parity gate

Linear: [SMA-524](https://linear.app/smaschek/issue/SMA-524/rust-paigasus-service-info-is-missing-its-moon-edge-to-paigasus-proto)
Related: [SMA-505](https://linear.app/smaschek/issue/SMA-505) (added the crate), [SMA-438](https://linear.app/smaschek/issue/SMA-438)
(found the gap), [SMA-389](https://linear.app/smaschek/issue/SMA-389) (D3 — the rule this spec corrects),
[SMA-409](https://linear.app/smaschek/issue/SMA-409) / [SMA-429](https://linear.app/smaschek/issue/SMA-429)
(the guard being extended; F3 is the gap § 4 closes)

Revised twice: once after an adversarial challenge, once after a spike that overturned the premise.
§ 10 records both.

## 1. Summary

Three Moon graph edges around `paigasus-service-info` are missing, so proto changes never retest it and
its own changes retest nothing. Fixing those edges is the issue as filed. Two investigations changed
the shape of the work:

1. **The guard cannot see half the fix.** `moon query projects --affected` follows `dependsOn` and is
   structurally blind to task-level `^:build`. A project-level guard case would have left half of this
   fix protected by nothing (§ 4).
2. **The repo's stated premise is wrong.** Moon 2.3.2 *does* auto-infer Rust edges — from `path =`
   deps, but not from `workspace = true` inheritance (§ 3). This is why the drift is scattered rather
   than systematic, and it makes a generic parity gate cheap enough to build here.

So this issue ships the edges, a parity gate that makes the whole class impossible, and corrections to
the premise everywhere the repo records it.

## 2. Measurement provenance

Every number was produced in a clean worktree at `origin/main` (`4546c6a`) with the repo-pinned moon
resolved through the proto shim, **not** the global binary — SMA-429's review finding F1 was that these
sets had been grounded on the wrong moon, and strict equality is version-coupled by design:

```
$ which moon
/Users/smaschek/.proto/shims/moon
$ moon --version
moon 2.3.2
```

Reproduce a project row with:

```bash
printf '%s\n' <touched-file> \
  | moon query projects --affected --downstream deep \
  | python3 -c 'import sys,json; print(", ".join(sorted(p["id"] for p in json.load(sys.stdin)["projects"] if p["id"]!="repo")))'
```

## 3. The premise is wrong, and precisely backwards

CLAUDE.md, SMA-389, `rs/crates/libs/paigasus-proto/moon.yml:7-10`,
`rs/crates/libs/paigasus-kernel-parity/moon.yml:7-10`, and the SMA-524 issue text all state:

> Cargo path deps are NOT auto-synced into Moon's graph in this repo — every in-tree edge is
> hand-declared.

**`path` deps are exactly the ones that ARE synced.** `moon query projects` labels every resolved
dependency `source=explicit` (hand-declared in `dependsOn`) or `source=implicit` (derived by the
toolchain):

| Edge | Cargo form | `source` |
| --- | --- | --- |
| `gateway → logging`, `iam → logging`, `iam → iam-core` | `path = "../../libs/…"` | **implicit** |
| `gateway/iam → proto`, `→ observability`, `iam-core → kernel` | `workspace = true` | explicit *(only because hand-declared)* |
| `gateway → kernel` | *no Cargo dep at all* | explicit *(phantom — § 5, A2)* |
| `service-info → proto` | `workspace = true`, not hand-declared | **absent** |

**Controlled proof.** Changing *only* `paigasus-service-info/Cargo.toml`'s dep form from
`workspace = true` to `path = "../paigasus-proto"`, with `moon.yml` untouched, makes the edge appear as
`source=implicit` and closes hole A by itself:

```
before:  paigasus-service-info-rs -> contracts (implicit)
after :  paigasus-service-info-rs -> contracts (implicit), paigasus-proto-rs (implicit)
         proto-derive->proto now includes paigasus-service-info-rs
```

**The corrected rule.** Moon 2.3.2's Rust toolchain resolves `path` deps into the project graph and
does **not** resolve `workspace = true` inheritance. So:

- a `path` dep needs no `dependsOn`;
- a `workspace = true` dep **must** be hand-declared;
- **either way `^:build` is still required** — inference gives the project edge, only `^:build`
  schedules the tasks (measured: with the implicit edge and no `^:build`, `service-info:build`/`:test`
  are still not scheduled by a proto edit).

This explains the scatter: crates using `path` got their edges free and nobody noticed the difference.

**We keep `workspace = true` and hand-declare.** Switching dep forms to harvest inference would fight
the workspace dependency table that exists to unify versions, and would be a repo-wide convention
change. D3 records this.

## 4. The guard cannot see half the invariant — SMA-429 F3, measured

Three variants of `paigasus-service-info/moon.yml`, each queried both ways:

| Variant | `projects --affected` (what the guard asserts) | `tasks --affected` (what CI runs) |
| --- | --- | --- |
| `dependsOn` + `^:build` (the fix) | present | `service-info-rs:build`,`:test` present |
| `dependsOn` only, no `^:build` | **present** | **ABSENT** |
| `^:build` only, no `dependsOn` | absent | — |

**Row 2 is the trap.** Delete the `^:build` and *every* affected-graph case stays green while
`moon ci --include-relations` under-builds exactly as it does today. The project guard walks the
project graph; `^:build` creates no project edge — it *consumes* one.

This is the "query-depth ↔ build-depth equivalence" gap SMA-429 logged as F3
(`docs/superpowers/specs/2026-06-16-sma-429-affected-graph-completeness-guard-design.md:171-177`) and
deferred. It bites here, so it is closed here — by assertion A3 of the parity gate (§ 6) and by one
behavioral task case (§ 7).

## 5. The complete, verified failure set

Produced by a prototype of the parity gate (§ 6) run against `origin/main`:

| Assertion | Count | Rows |
| --- | --- | --- |
| **A1** Cargo dep absent from Moon graph | 3 | `gateway → service-info`, `iam → service-info`, `service-info → proto` |
| **A2** explicit Moon edge with no Cargo backing | 1 | `gateway → kernel` |
| **A3** in-tree deps but no `^:build` | 2 | `service-info-rs:build`, `service-info-rs:test` |

A1 rows under-build (silent correctness hole). The A2 row over-builds — a cost, never a correctness
risk. That asymmetry is why A2 gets an allowlist rather than a fix (D4).

### 5.1 The gate must use `cargo metadata`, not a regex

A first prototype regex-matched `^paigasus-\S+\s*=` in `Cargo.toml` and reported **6** A2 rows. Five
were false positives: the repo also uses TOML **dotted keys** —

```toml
paigasus-proto-derive.workspace = true      # rs/crates/libs/paigasus-proto/Cargo.toml:24
paigasus-kernel.workspace = true            # rs/crates/bindings/paigasus-py-bindings/Cargo.toml:22
```

— which that pattern cannot see. A hand-rolled parser would have shipped a gate that flagged five real
edges as phantom. The gate resolves dependencies with `cargo metadata --no-deps --format-version 1`,
which is authoritative across every declaration form. **Non-negotiable** (D5).

## 6. The parity gate

`assert_cargo_moon_parity`, added to `ci/affected-graph/run.sh`. It compares Cargo's dependency graph
(`cargo metadata`) against Moon's *own resolved* graph (`moon query projects`) — it never parses
`moon.yml` for edges, so it is immune to formatting and inherits Moon's `implicit`/`explicit`
resolution for free.

- **A1** — every in-tree Cargo dep must appear in the crate's Moon dependency set, by either source.
  Catches under-building.
- **A2** — every `explicit` Moon edge must have Cargo backing, unless allowlisted. Catches phantom
  edges and stale hand-declarations. `contracts` is exempt: it is a build-scope parent from
  `contracts:generate`, not a Cargo dep.
- **A3** — every crate with in-tree Cargo deps must carry `^:build` on `build` **and** `test`. This is
  the generic form of § 4's fix, and the only assertion in the repo that can see the `^:build` half.

The allowlist is a single explicit table with a required reason string, so an entry is a recorded
decision rather than a silent exemption:

```bash
# <consumer> -> <upstream>: why this explicit edge has no Cargo backing.
PARITY_ALLOW_NO_CARGO="paigasus-gateway-rs->paigasus-kernel-rs"
```

Each assertion gets a `--negative-control` entry (D6): the harness must red when a synthetic violation
is injected, so a green gate is meaningful rather than vacuous.

## 7. Changes

1. **`rs/crates/libs/paigasus-service-info/moon.yml`** — `dependsOn: ['paigasus-proto-rs']` + `^:build`
   on `build`/`test`, commented with the corrected § 3 rule.
2. **`rs/crates/services/paigasus-iam/moon.yml`**, **`…/paigasus-gateway/moon.yml`** — each gains
   `paigasus-service-info-rs` in `dependsOn`; both already carry `^:build`. The gateway file records
   its allowlisted kernel edge (D4).
3. **`ci/affected-graph/run.sh`** —
   - `assert_cargo_moon_parity` (§ 6) + allowlist + three negative controls;
   - `proto-derive->proto` expected set gains `paigasus-service-info-rs`;
   - SMA-438's *"`paigasus-service-info-rs` is deliberately ABSENT"* paragraph deleted, **together with
     the bare `#` separator preceding it**;
   - new project case `service-info->services`, touched file
     `rs/crates/libs/paigasus-service-info/src/lib.rs` (matching all six existing cases);
   - one behavioral task case `proto->service-info-tasks` proving § 4's semantics — the parity gate
     asserts `^:build` is *declared*, this asserts it *schedules*;
   - `contracts->proto` and `kernel->bindings` expected sets **unchanged** (§ 8).
4. **`ci/affected-graph/README.md`** — document the new cases and the parity gate, and fix three
   pre-existing errors: the contracts bullet omits `paigasus-iam-rs` and `paigasus-service-info-rs`
   (both live at `run.sh:108`), the kernel bullet omits `paigasus-iam-core-rs`/`paigasus-iam-rs`, and
   there is **no bullet at all** for `proto-derive->proto`. Nothing gates this file — noted in the PR.
5. **`CLAUDE.md`** — replace the wrong premise with § 3's corrected rule.
6. **`rs/crates/libs/paigasus-proto/moon.yml`**, **`…/paigasus-kernel-parity/moon.yml`** — correct the
   same wrong premise in both comment blocks, which is where SMA-389 D3 is quoted verbatim.

## 8. Unchanged, and proven so

| Case | Before | After |
| --- | --- | --- |
| `contracts->proto` | 7 projects | unchanged — service-info already reached via its own `contracts:generate` dep, the coincidence that masked hole A |
| `kernel->bindings` | 10 projects | unchanged — `iam`/`gateway` are graph leaves; a new *upstream* cannot change a `--downstream deep` result |

## 9. Decisions

**D1 — Fix all four under-building defects** (A1×3 + A3×2 on one crate), not just the one the issue
names. Same root cause, same crate, one audit.

**D2 — Build the parity gate in this PR.** Originally deferred on a cost argument that the spike
falsified: the infrastructure already exists (`repo:affected-smoke` declares `rs/**/Cargo.toml` and
`rs/crates/*/*/moon.yml` in `inputs`; `run.sh` ships a `--negative-control` harness), and § 3 showed the
gate can read Moon's resolved graph instead of parsing config. The deeper reason: the strict-equality
guard only asserts edges someone wrote a *case* for. SMA-505 added a crate with no case, which is why
this bug survived a full review. Per-case guarding fixes today's crate; A1–A3 fix the class.

**D3 — Keep `workspace = true`; hand-declare the edges.** Switching to `path` deps would harvest
inference (§ 3) but fights the workspace dependency table that unifies versions, and is a repo-wide
convention change. Out of scope; recorded as a possible future simplification.

**D4 — Allowlist the phantom `gateway → kernel` edge; do not remove it.** It over-builds, the safe
direction. Removing it changes the `kernel->bindings` expected set — an unrelated edit to a guard case
this issue declares out of scope. Allowlisting it with a reason string makes it a recorded decision;
an undocumented phantom edge is what made the first audit pass wrong.

**D5 — Resolve Cargo deps with `cargo metadata`, never a regex** (§ 5.1). A regex prototype produced
five false phantoms.

**D6 — Every assertion gets a negative control.** The gate's whole value is catching a silent hole; a
gate that passes vacuously reproduces the bug it exists to prevent.

**D7 — Do not fix `lint`/`fmt` propagation.** `.moon/tasks/rust.yml:25-30` gives `lint`
(`cargo clippy -- -D warnings`) and `fmt` no `deps`, so they propagate across **no** edge, repo-wide. A
proto edit schedules `proto-rs:lint` but gives downstream consumers only `build`/`test` — so a change
tripping `-D warnings` in a consumer still reds `main` after merge. Pre-existing and affecting all 13
crates; fixing it here would change the task graph for every crate under cover of an edge fix.
Documented + follow-up.

## 10. Verification

1. `bash ci/affected-graph/run.sh` — strict equality re-verifies **every** case, and the parity gate
   must report A1/A3 empty and A2 reduced to the allowlisted row.
2. `bash ci/affected-graph/run.sh --negative-control` — pre-existing controls plus one per new
   assertion (D6).
3. **Bite checks**, each must fail then be restored:
   - revert the three `dependsOn` edges → A1 reds, project cases red;
   - revert **only** the `^:build` → project cases stay **green**, A3 reds and the task case reds. This
     is the check that proves § 4's hole is closed rather than merely described;
   - drop the allowlist entry → A2 reds.
4. The full gate graph per `CLAUDE.md`. Note § 11: this proves the repo still builds, **not** that the
   new edges work.

## 11. This PR's own CI exercises only the guard

The PR touches `moon.yml` files, `run.sh`, `README.md`, `CLAUDE.md`, and this spec. Measured, that set
affects exactly one task: `repo:affected-smoke`. No Rust `build`/`test` runs, because
`.moon/tasks/rust.yml:18` gives `build` the inputs `['@group(sources)', 'Cargo.toml']` — a project's own
`moon.yml` is not among them. So the guard is the only thing that tests this change, which is the
argument for strengthening it here rather than leaning on the gate graph.

## 12. Cost accepted

Any edit to `paigasus-service-info/src/lib.rs` now schedules `paigasus-iam-rs:test` — the Docker-backed
testcontainers suite (Postgres/Redis/NATS) in a job with `timeout-minutes: 30` and a documented
disk-exhaustion history (`.github/workflows/ci.yml:22-29`). That is the point of the fix, not a
regression. Recorded so it is not later mistaken for one.

## 13. Rollback

`git revert` of the single commit. Recorded because strict equality means a wrong expected set reds
`main` for every contributor until reverted, and `repo:affected-smoke` is a required check.

## 14. Follow-ups to file

- `lint`/`fmt` propagate across no edge, repo-wide (D7).
- Possible simplification: migrate in-tree deps from `workspace = true` to `path` and delete the
  hand-declared `dependsOn` edges inference would then supply (D3).

## 15. What the reviews changed

**The adversarial challenge** found two blockers. The audit was one-directional and its "the only drift
is …" conclusion was **false** — it missed the phantom `gateway → kernel` edge, and the decision to
defer the parity gate rested on that wrong count. More consequentially, it showed the proposed guard
asserted only the `dependsOn` half of the invariant, so the `^:build` half would have shipped protected
by nothing. §§ 4, 6 and the bite checks are downstream of that. It also caught the stale README, an
unrunnable repro, missing measurement provenance, the `lint`/`fmt` gap, the unspecified touched-file
argument, and an over-claim about one-directionality.

**The spike** — which the challenge asked for and the first revision deferred — overturned the premise
the issue, CLAUDE.md, SMA-389 and two `moon.yml` comments all state (§ 3). That flipped the
defer-the-gate decision to build-it-now (D2) and added the documentation corrections in § 7.5–7.6.

**Prototyping the gate before planning it** caught the dotted-key parser bug (§ 5.1) that would have
shipped a gate flagging five sound edges as phantom.
