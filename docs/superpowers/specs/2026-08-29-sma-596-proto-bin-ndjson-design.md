<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-596 — proto's agent-mode NDJSON breaks `$(proto <subcommand> …)`

Date: 2026-08-29
Issue: SMA-596
Branch: `feature/sma-596-proto-bin-ndjson`

Revision 3. Revision 1 was reworked after an adversarial review; revision 3 pulls the
sibling module's fallback (formerly residual L5) into scope on Sven's instruction. §10
records what changed and why.

## 1. The defect

`proto` prints NDJSON on **stdout** when it detects an agent environment. It emits a
preamble line first:

```
{"type":"message","message":"Detected an AI agent environment, printing as NDJSON. …"}
{"type":"message","message":"/Users/…/.proto/tools/release-plz/0.3.158/release-plz"}
```

`ci/release-parity/ecosystems/release-plz.sh:16` captures that:

```bash
RELEASE_PLZ_BIN="$( (cd "$_RP_REPO_ROOT" && proto bin release-plz) 2>/dev/null || command -v release-plz || echo release-plz )"
```

`RELEASE_PLZ_BIN` becomes a two-line JSON blob, not a path. Three properties make the
failure silent at the point it occurs:

- The noise is on **stdout**, so `2>/dev/null` does not remove it.
- `proto bin` exits **0**, so neither `||` fallback runs. proto succeeded. It answered
  in a different language.
- Nothing validates the captured value, so the error surfaces 87 lines later, at line
  103, as `No such file or directory`.

Detection keys on `AI_AGENT`, `CLAUDECODE` and `CLAUDE_CODE_ENTRYPOINT`. CI sets none
of them, so CI has never seen this and CI verdicts are unaffected.

**This is a property of the proto CLI's reporter, not of `proto bin`.** Any
stdout-producing `proto` subcommand carries it. The audit (§3) and the documentation
change (D5) are scoped accordingly, not to `proto bin` alone.

### 1.1 Acceptance criteria (verbatim from SMA-596)

1. The three `repo:release-parity*` gates run to a real verdict in an agent session,
   with no environment variables unset by hand.
2. Proven: run them in an agent session before the fix (inconclusive, rc=2) and after
   (pass), and record both.
3. A malformed binary resolution fails at the point of resolution with a readable
   message, not downstream.
4. Every other `$(proto bin …)` capture in `ci/` is audited and either fixed or shown
   not to be affected.
5. CLAUDE.md's SMA-595 gotcha is updated to point at the fix instead of the unset
   workaround.

AC1 is satisfied in a way the issue did not anticipate: two of the three gates already
run to a real verdict (§2). §6 labels every verification step with the AC it discharges.

## 2. Measured scope — one gate, not three

SMA-596's body and CLAUDE.md's SMA-595 gotcha (lines 52-59) both state that all three
`repo:release-parity*` gates abort. **That is wrong**, and it is a regression against
this repo's own earlier measurement:
`docs/superpowers/specs/2026-08-22-sma-530-release-parity-negative-control-design.md:71-73`
already recorded the defect release-plz-specifically — "breaks `RELEASE_PLZ_BIN`
resolution at `ecosystems/release-plz.sh:16` and yields rc 2". CLAUDE.md widened that
correct, narrow finding into a false one.

Measured on this branch at `origin/main` `3a211eb`, in **one** agent session, with
`AI_AGENT`, `CLAUDECODE` and `CLAUDE_CODE_ENTRYPOINT` all set and none unset by hand —
the three rows are from the same session under identical environment conditions:

| Gate | ecosystem | rc | Result |
| ---- | --------- | -- | ------ |
| `repo:release-parity` | release-plz | **2** | `line 103: {"type":"message",…}: No such file or directory` → `negative-control INCONCLUSIVE: infrastructure error (rc=2)` |
| `repo:release-parity-py` | python-semantic-release | 0 | `negative-control OK: harness reported red as expected` |
| `repo:release-parity-ts` | semantic-release | 0 | `negative-control OK: harness reported red as expected` |

The mechanism, stated precisely. `ci/release-parity/run.sh:21` sources exactly **one**
ecosystem module per invocation, and only `release-plz.sh` invokes the **proto CLI
itself**. The other two invoke proto-*shimmed* tools — `python-semantic-release.sh:28`
captures the stdout of `uv run …`, `semantic-release.sh:52` the stdout of `node -e …`,
and both `uv` and `node` do resolve to `~/.proto/shims/…` (measured). A shim **execs**
the tool, so the captured stdout is the tool's, not proto's reporter's.

That distinction matters for the next author. The rule is **not** "a shimmed tool is
safe to capture" — this work did not test that generally. The rule is "the `proto` CLI's
own stdout is unsafe to capture". The rc=0 rows above are what carries this section; the
mechanism explains them.

This correction is part of the deliverable. Leaving the "all three" claim in CLAUDE.md
would keep sending the next reader to look for two failures that do not exist.

## 3. Audit of every proto-CLI capture (AC4)

Repo-wide `grep -rn 'proto bin'`, excluding `docs/`, returns **one** hit:
`release-plz.sh:16`. Widening to any captured `proto <subcommand>` adds no hits outside
`docs/`. There is no second call site to fix.

`CARGO_BIN` (`release-plz.sh:19-20`) is **not** affected by the NDJSON defect: it uses
`command -v cargo`, which resolves to `~/.cargo/bin/cargo` — rustup's proxy, not a proto
shim — so no proto output is ever captured. Measured. It is, however, in scope for D2
for a different reason; see D2.1.

## 4. Decisions

### D1 — Resolve with proto's own reporter flag

`proto --help` documents a global `-r, --reporter <text|json|ndjson>` option, env
`PROTO_REPORTER`. Measured in this agent session, with the three detection variables
set, both forms return the bare path:

```
$ proto --reporter text bin release-plz     → /Users/…/.proto/tools/release-plz/0.3.158/release-plz
$ PROTO_REPORTER=text proto bin release-plz → /Users/…/.proto/tools/release-plz/0.3.158/release-plz
```

**Chosen: the flag**, not the env var. The flag scopes the change to this one call. An
exported `PROTO_REPORTER` would leak into every child process the harness spawns,
including release-plz's own `cargo` subprocesses, for no benefit.

**Rejected: parsing the NDJSON back out.** Reading the last line and unwrapping
`.message` would survive proto dropping the flag, but it is more code, needs a JSON
reader, and must handle two output shapes. D2's assertion already converts that
regression into a loud, immediate failure, which is the outcome the parsing would buy.

**What guarantees the flag exists — and what does not.** `.prototools:4` pins
`proto = "0.61.1"`, but that pin does **not** determine which binary a bare `proto` call
in a shell script executes. CLAUDE.md:46-49 records the mechanism: proto is read at the
fixed path `~/.proto/bin/proto`, and neither `PATH` order nor the `.prototools` pin
overrides that. Measured here: `command -v proto` → `~/.proto/bin/proto`, not a shim.

So the honest statement is: an older local proto without `--reporter` exits non-zero,
and **D2's first arm reports that as a readable rc=2**. The assertion is the guarantee;
the pin is not. Which proto version a CI runner actually executes after
`moonrepo/setup-toolchain` is **not measured by this work** (§7 L3).

### D2 — Assert every binary resolution, at the point of resolution

Immediately after each capture, assert the result is an executable file. On failure,
print the file, the resolved value and the likely cause, and exit **2**.

Exit 2 is not arbitrary: `run.sh` already distinguishes rc=2 (the harness could not run)
from rc=1 (the gate reported red), and SMA-530's negative controls are built on that
distinction. A resolution failure is an infrastructure failure and must keep reporting
as one.

**The failure message must carry the rc=2 classifier itself.** This is not cosmetic. The
module is sourced at `run.sh:21`, so an `exit 2` fires *during the source* and `run.sh`
never reaches the lines that print `negative-control INCONCLUSIVE: infrastructure error
(rc=2)` (`run.sh:67`) or `== parity ABORTED: infrastructure error on case … ==`
(`run.sh:81`). Those are the strings CLAUDE.md:56 teaches readers to grep for. The
module's own FATAL text therefore has to speak the same vocabulary, or this change makes
the abort *harder* to triage than the bug it fixes.

Sourcing order is safe: `run.sh:12` parses arguments, `:15` exits on `-h|--help`, and
`:21` sources the module. So an `exit` from module top level cannot pre-empt argument
handling or help output. It does mean a contributor who runs
`source ci/release-parity/ecosystems/release-plz.sh` by hand loses their interactive
shell — that is the correct trade for a module `run.sh` sources at top level under
`set -e`, and it must be commented so nobody "fixes" it to `return`.

#### D2.1 — `CARGO_BIN` is asserted too

`release-plz.sh:19-20` resolves cargo with a **presence** test (`[ -n "$CARGO_BIN" ]`)
and an unconditional fallback to `$HOME/.cargo/bin/cargo`, a path that need not exist.
That reproduces exactly the failure shape §1 complains about: a bad value surfaces at
line 103 as a cargo error rather than as a resolution error.

D2's contract is "assert the resolution, at the point of resolution". Applying it to one
of the two resolutions in the same file would leave the asymmetry unexplained in the
finished code. `CARGO_BIN` gets the same `[ -x ]` assertion. Its fallback is **kept** —
unlike D3's, it is a real, reachable default rather than dead code, and cargo is not the
tool under test.

#### D2.2 — What `[ -x ]` does and does not assert

Executability, not identity. `[ -x ]` would pass for `~/.proto/shims/release-plz` too.
That is deliberate: D3 removes the fallbacks, so there is no path by which a substituted
binary can be resolved — the assertion exists to catch a **malformed capture** (the JSON
blob), not to authenticate the binary. Asserting the version against `.prototools:15`
was considered and rejected: it costs a subprocess and new coupling to close a hole that
D3 closes structurally.

### D3 — Drop `RELEASE_PLZ_BIN`'s fallback chain

`|| command -v release-plz || echo release-plz` is removed.

Neither fallback has ever executed **in an agent session**, because `proto bin` exits 0.
Reinstating them behind the fixed primary would be worse than leaving them dead:

- `command -v release-plz` resolves to `~/.proto/shims/release-plz` (measured). The
  script's own header comment (`release-plz.sh:11-15`) records that a shim resolves its
  version by walking up from CWD to find `.prototools`, and that this fails from the
  fixture directory in CI with `proto::tool::unknown_id`. **Measured here: the shim does
  work from `/tmp` on this machine and returns the pinned 0.3.158**, and there is no
  global `release-plz` pin in `~/.proto/.prototools` to explain the difference. So the
  header's claim is unreproduced locally. This spec does not assert the shim is broken —
  only that its behaviour differs between environments and is therefore not a
  trustworthy fallback for a version-comparison harness.
- `echo release-plz` is not a resolution at all. It defers to `PATH` and hopes.

The harness exists to compare one specific pinned release-plz's classification behaviour
against the other two ecosystems. A silently substituted binary — a different version,
or a shim that resolves differently in CI than locally — produces a parity verdict about
the wrong thing. Failing loudly is the correct behaviour for this file.

**Accepted risk, stated plainly.** "Neither fallback has ever executed" is measured in an
agent session only. This work has **no evidence about which arm CI takes**. If
`proto bin release-plz` fails on a CI runner for any reason, the `|| command -v` arm
silently carries the gate today, and after this change the gate hard-fails rc=2. §6
step 6 exists to reduce that risk before merge; it does not eliminate it, because it runs
on this machine, not on a runner. `.github/workflows/ci.yml:76` runs a bare
`proto install`, so the primary arm is expected to resolve — expectation is not the
standard used elsewhere in this repo, and §7 L3 records it as such.

Accepted cost: a contributor without proto installed now gets a hard error instead of a
run that might have worked. `CONTRIBUTING.md` already makes `proto install` the first
step of local setup.

#### D3.1 — `PSR_BIN`'s fallback goes too

`python-semantic-release.sh:29` is `[ -n "$PSR_BIN" ] || PSR_BIN="$( command -v
semantic-release 2>/dev/null || echo semantic-release )"`. Unlike release-plz's dead
fallbacks this one is genuinely **reachable**: line 28 ends in `|| true`, so any failure
of `uv run --frozen` leaves `PSR_BIN` empty and hands the harness whatever
`semantic-release` happens to be on `PATH` — or, failing that, the bare string.

D3's argument applies to it word for word: a version-comparison harness must not
silently substitute the tool under test. This module is the *reference* implementation
for the 0.x expectation the other two are compared against, so a substituted binary here
corrupts the comparison rather than one side of it.

**Measured on this branch, in this fresh worktree, before any change:**

| Arm | Result |
| --- | ------ |
| line 28, `uv run --frozen …` | rc=0 → `…/sma-596/py/.venv/bin/semantic-release` |
| line 29, `command -v semantic-release` | **not on `PATH`** |
| `py/.venv` present? | yes — line 28 bootstrapped it; `uv sync` was never run here |

Two things follow. The fallback is **not** carrying the py gate today, so removing it
cannot break the currently-green result (§2's rc=0 row). And if it ever did fire on this
machine it would fall through to `echo semantic-release`, producing a bare string that
fails at execution — later, and less readably, than an assertion would.

So: drop the `command -v`/`echo` fallback, keep the `|| true` on line 28 only long
enough to produce an empty value, and assert `[ -x "$PSR_BIN" ]` with the same rc=2
classifier and the same message shape as D2. On a machine where `semantic-release` *is*
globally installed — the case this repo has no control over — this converts a silent
wrong-tool run into a loud failure.

**Accepted cost, and it is larger here than for release-plz.** Line 28's own comment
records that `uv run --frozen` bootstraps `py/.venv` from `uv.lock`. A contributor with a
broken `py/` toolchain previously got a fallback; now they get rc=2. That is the same
trade D3 makes, applied to a module where the primary arm does more work. §6 step 9
exercises it.

### D4 — No new `repo:*` gate

Considered and rejected for this issue. A gate scanning `ci/` for an unguarded captured
`proto` invocation would cost the full ritual — the `ci.yml` `T=(…)` array, CLAUDE.md's
marker-delimited command, an `affected-smoke` re-baseline, self-tests and a negative
control — to guard **one** line of code that this change is deleting the hazardous form
of.

The consequence is that nothing continuously proves the new assertion can fire. §6
step 3 proves it once, by hand, and reverts the mutation. That residual is accepted and
recorded (§7 L1, L6).

### D5 — Correct CLAUDE.md rather than delete the entry

CLAUDE.md lines 52-59 carry the NDJSON gotcha with three things now wrong or stale: the
"all three `repo:release-parity*` gates abort" scope claim (§2), the `unset AI_AGENT
CLAUDECODE CLAUDE_CODE_ENTRYPOINT` workaround that this change removes the need for
**when running the release-parity gates** (CLAUDE.md:49-51 records a second, unrelated
agent-session proto oddity, so the unset is not obsolete repo-wide), and the framing of
the hazard as specific to `proto bin`.

The entry stays, because the underlying proto behaviour is unchanged and still breaks
any *new* captured `proto <subcommand>` a future author writes. It is rewritten to:

- state the behaviour and that it applies to any stdout-producing proto subcommand;
- name `--reporter text` as the fix, with `release-plz.sh` as the worked example;
- state the corrected scope (one gate, and why);
- carry D4's repo-wide residual (§7 L1), because CLAUDE.md is where a person writing a
  new gate under `ci/foo/` will actually look.

The cross-reference at line 84 ("The NDJSON entry above is the same root tool, a
different symptom") stays valid and is left alone.

## 5. The change

Two functional files.

**`ci/release-parity/ecosystems/release-plz.sh`, lines 11-20.** The contract:

- Resolve `RELEASE_PLZ_BIN` with `proto --reporter text bin release-plz`, no fallbacks.
- Assert `[ -x ]` on `RELEASE_PLZ_BIN` and on `CARGO_BIN`.
- On failure exit 2, with a message that (a) names the file, (b) prints the resolved
  value, (c) names the likely cause, and (d) **carries the `infrastructure error (rc=2)`
  classifier** so it matches the vocabulary `run.sh:67`/`:81` would have used.
- Keep one sentence of the existing header explaining why the absolute path is resolved
  at all — the shim's CWD-relative version lookup — marked as unreproduced locally per
  L2. Without it a future author deletes the `_RP_REPO_ROOT` dance as redundant and
  reintroduces the SMA-398 bug.
- Keep the comment's plural ("binaries"): it covers both resolutions.
- Comment the `exit`-from-a-sourced-module choice, per D2.

**`ci/release-parity/ecosystems/python-semantic-release.sh`, lines 26-29.** Per D3.1:
drop the `command -v`/`echo` fallback and assert `[ -x "$PSR_BIN" ]`, with the same rc=2
classifier and message shape. Keep line 28's `|| true` — an empty value is what the
assertion reports on. Note in the comment that this module is the reference
implementation for the 0.x expectation, which is why substitution matters more here.

Exact wording is the plan's business. Error messages must not cite line numbers — they
move as the comment block grows. The two modules' assertions should read alike; a reader
comparing them must not have to wonder whether a difference is meaningful.

Documentation changes:

- `CLAUDE.md` — rewrite the NDJSON bullet per D5, including L1.
- `ci/release-parity/README.md` — record that both ecosystem modules now resolve their
  tool with no fallback and assert it, so the policy is stated once for the harness rather
  than inferred from two files. L1 goes to CLAUDE.md, not here: the README's Limitations
  section is about the negative-control pins, and a repo-wide residual filed there would
  not be found by someone writing a new gate elsewhere.

## 6. Verification

There is no unit-test layer under `ci/release-parity/`; the harness verifies by running.
Every step names its exact command, because the gate-level and script-level paths differ
in CWD, environment and caching, and would otherwise be two different experiments.

**Precondition for steps 1-4:** confirm `AI_AGENT`, `CLAUDECODE` and
`CLAUDE_CODE_ENTRYPOINT` are visible *inside the task process*, and that `PROTO_REPORTER`
is unset in the environment — otherwise the steps pass for the wrong reason.

1. **AC1 + AC2 — after, at gate level, in an agent session.**
   `moon run repo:release-parity --force`, then the same for `-py` and `-ts`.
   `--force` is required: a restored file re-hits a cached PASS.
   `repo:release-parity` must move rc=2 → rc=0. The other two must stay rc=0 — a
   regression there would mean the change reached a module it should not have. The
   "before" half of AC2 is the §2 table, already recorded.
2. **AC1 — the real suites, not only the controls.**
   `bash ci/release-parity/run.sh`, `… --ecosystem python-semantic-release`,
   `… --ecosystem semantic-release`, each without `--negative-control`. The moon task
   always runs both lines (`moon.yml:83-86`), so this is only reachable by calling the
   script directly. A control proves the harness can report red; only the real run proves
   the parity assertion itself still holds.
3. **AC3 — prove the assertion bites, by mutation.**
   Replace the `proto` call with an `echo` of a JSON blob, run
   `bash ci/release-parity/run.sh --negative-control`, and confirm the failure is the new
   message naming **the file and the resolved value** and carrying the rc=2 classifier —
   not `line 103: … No such file or directory`. Then restore by reverting the edit, not
   by moving a `.bak` file back: a backwards mtime makes cargo serve a stale artifact and
   the re-run then fails for an unrelated reason.
4. **AC3 — prove the flag is what fixes it, not the assertion.**
   Remove `--reporter text` alone, with the fix otherwise in place, and confirm the
   assertion reports the JSON blob. Without this step, step 3 cannot distinguish "the
   assertion fires" from "the flag works". Restore.
5. **AC3 — the same two steps for `CARGO_BIN`,** at least once: point it at a
   nonexistent path and confirm the new assertion fires at resolution rather than at
   line 103.
6. **D3's risk — exercise the non-agent code path.**
   `env -u AI_AGENT -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT -u PROTO_REPORTER bash
   ci/release-parity/run.sh` and `env -u … proto bin release-plz; echo $?`. Unset
   `PROTO_REPORTER` too: an inherited `text` would make the probe pass for the wrong
   reason, proving nothing about the flag in the code. CI runs this shape and has never run
   the new one. This does not prove CI behaviour (§7 L3) but it removes the case where
   the change is broken for every non-agent caller.
7. **AC5 — read back the rewritten CLAUDE.md bullet** against §2 and D5 and confirm no
   "all three" claim survives anywhere in the file.
8. **D3.1 — the py module's assertion, both directions.**
   Point `PSR_BIN` at a nonexistent path and confirm the new assertion fires at
   resolution with the rc=2 classifier, then restore. Then confirm the unmutated module
   still resolves through arm 1: `(cd py && uv run --frozen python -c 'import
   shutil,sys; sys.stdout.write(shutil.which("semantic-release") or "")')` must print a
   path. The measured baseline is `…/py/.venv/bin/semantic-release`, with
   `semantic-release` absent from `PATH` — so a green py gate after this change proves
   arm 1 carried it, not the deleted fallback.
9. **Full graph.** `moon ci` over CLAUDE.md's documented target list.
   `ci/release-parity/**/*` is an input to all three gates *and* to
   `repo:affected-smoke` (`moon.yml:196`), so this change re-runs the gate that holds the
   pins discussed in §9.

## 7. Limitations, stated

- **L1 — a future captured `proto <subcommand>` is still unguarded.** D4's accepted
  residual. Nothing reds if a second call site is written the broken way. The mitigation
  is the rewritten CLAUDE.md entry, which is where such an author will look.
- **L2 — the shim fallback's CI behaviour is recorded, not reproduced.** D3 cites the
  file header's `proto::tool::unknown_id` note for CI, which this work did not reproduce
  and in fact could not reproduce locally. The decision to drop the fallback does not
  depend on that claim being true; it rests on the harness needing a known-version
  binary.
- **L3 — nothing here is measured on CI, or on Linux.** Three specific gaps: which proto
  binary a runner executes after `moonrepo/setup-toolchain`; whether `--reporter text`
  exists on it; and which resolution arm CI takes today. §6 step 6 exercises the
  non-agent *code shape* on this machine, which is not the same thing. `--reporter text`
  is verified on proto 0.61.1, macOS, only.
- **L4 — the new code path has never run on CI.** D1 adds a flag and D3 deletes two
  fallbacks; both are on the line CI executes. The first CI run of this branch is the
  first execution of that path anywhere. This is the risk §6 step 6 reduces and does not
  remove.
- **L5 — the assertions have no continuous coverage.** §6 steps 3 and 8 prove they can
  fire, once each, by hand, then revert. D4 declined the gate that would keep proving it.
  A future edit that neuters either assertion would not red.
- **L6 — `semantic-release.sh` keeps a different resolution style, unreviewed.** The ts
  module invokes `node` directly against a runner script rather than resolving a tool
  binary into a variable, so D2's "assert the resolution" contract has no obvious site
  there. It was not examined for an equivalent hazard. Two of three modules now share a
  policy; the third was not brought into it, and that is a gap rather than a decision.

## 8. Out of scope

- Any change to `semantic-release.sh`. §2 measures it as unaffected by the NDJSON defect,
  and L6 records that its different resolution style was not reviewed for an equivalent
  hazard. `python-semantic-release.sh` **is** in scope, per D3.1 — it is unaffected by the
  NDJSON defect but carries the live fallback D3 argues against.
- A `repo:*` gate for captured proto invocations. D4.
- Asserting `RELEASE_PLZ_BIN`'s *identity* (version) rather than its executability.
  D2.2.
- SMA-597 (`ciReport.json` carries no stdout/stderr). SMA-597 names this issue as
  sharing a root tool and asks whether they share a fix. They do not. SMA-597's recorded
  case is a proto **shim exec failure** (`Permission denied (os error 13)`) — and
  CLAUDE.md:69-78 is explicit that *why* the shim is briefly non-executable **is still
  unknown**, measured on one session only. So the two are related by tool, not by cause,
  and SMA-597's deliverable is a corrected diagnosis procedure plus a plan-template fix.
  No line of this change affects it.

## 9. Interaction with the pin registry

Stated here rather than left to §6 step 8. `RELEASE_PARITY_SH_CALL_SITES`
(`ci/affected-graph/ci_targets.py`) pins five literal lines inside
`ci/release-parity/run.sh`, and `SELF_SCHEDULED_GATES["release-parity*"]` pins the three
tasks' `moon.yml` invocation lines. **This change touches neither file**, so no pin moves
and no re-baseline is needed. `repo:affected-smoke` lists `ci/release-parity/**/*` among
its inputs, so the change does correctly select the gate that holds those pins.
`repo:input-liveness` is unaffected: no glob or declared file changes.

## 10. What revision 2 changed, and why

An adversarial review of revision 1 returned NEEDS REWORK. Every finding below was
verified against the repo before being folded in; none was accepted on assertion alone.

**Fixed:**

- The acceptance criteria were never written down (§1.1 now quotes them; §6 labels each
  step).
- §7 L4 claimed CI "exercises the same code path it always did". False — D1 and D3 both
  change that line. L4 rewritten, L3 added, and §6 step 6 added to exercise the non-agent
  shape.
- §2's mechanism was wrong. `uv` and `node` *are* proto shims (measured), so "neither
  captures proto's stdout" was false. The real rule — only `release-plz.sh` invokes the
  proto CLI itself — is now stated, with an explicit warning not to generalise it to
  shimmed tools.
- D1 inferred flag availability from `.prototools`. CLAUDE.md:46-49 disproves that. The
  reasoning now rests on D2's assertion, and the CI proto version is recorded as
  unmeasured.
- `CARGO_BIN` was excluded on NDJSON grounds while D2's principle covered it. Now
  asserted (D2.1).
- The rc=2 diagnostic markers vanish when `exit 2` fires during `source`. The FATAL text
  now has to carry the classifier (D2).
- §6 did not say how to invoke each step; gate-level and script-level differ materially.
  Exact commands added, plus an environment precondition.
- Audit scope widened from `proto bin` to any captured proto subcommand, and the grep
  restated as repo-wide.

**Added as residuals rather than fixed:** L5 (the sibling module's live fallback) and L6
(the assertion has no continuous coverage). Both are real; both are outside the defect
this issue was filed for. L5 in particular is a one-line extension that was deliberately
not taken — see GATE 1.

**Also folded in:** the SMA-530 citation (§2), which turns "we re-measured and CLAUDE.md
is wrong" into the stronger "CLAUDE.md regressed against a prior measurement"; the D5
narrowing of what the `unset` workaround was for; keeping the header's shim note (§5);
moving L1 to CLAUDE.md rather than the harness README; and dropping line numbers from
error messages.

**Nothing was rejected.** Every finding was either folded in or recorded as a stated
residual.

### Revision 3

Sven instructed that residual L5 — the sibling module's live, unasserted fallback — be
brought into scope rather than recorded. D3.1 is the result, and it is a stronger
decision than revision 2's residual because the arms were measured first:

- Arm 1 (`uv run --frozen`) resolves to `py/.venv/bin/semantic-release` and bootstrapped
  `py/.venv` itself in this fresh worktree.
- Arm 2 (`command -v semantic-release`) finds nothing — the tool is not on `PATH` here.

So the fallback is not carrying the py gate, and removing it cannot break §2's rc=0 row.
Had the measurement gone the other way, D3.1 would have had to keep the fallback.

Two residuals were renumbered rather than dropped: the old L6 (no continuous coverage)
is now L5, and a new L6 records that `semantic-release.sh`'s different resolution style
was never reviewed for an equivalent hazard — two of three modules now share a policy and
the third was not brought into it.
