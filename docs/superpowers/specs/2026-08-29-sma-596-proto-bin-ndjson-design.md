<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-596 — proto's agent-mode NDJSON breaks `$(proto bin …)`

Date: 2026-08-29
Issue: SMA-596
Branch: `feature/sma-596-proto-bin-ndjson`

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

## 2. Measured scope — one gate, not three

SMA-596's body and CLAUDE.md's SMA-595 gotcha both state that all three
`repo:release-parity*` gates abort. **That is wrong.** Measured on this branch, in an
agent session, at `origin/main` `3a211eb`:

| Gate | ecosystem | rc | Result |
| ---- | --------- | -- | ------ |
| `repo:release-parity` | release-plz | **2** | `line 103: {"type":"message",…}: No such file or directory` → `negative-control INCONCLUSIVE: infrastructure error (rc=2)` |
| `repo:release-parity-py` | python-semantic-release | 0 | `negative-control OK: harness reported red as expected` |
| `repo:release-parity-ts` | semantic-release | 0 | `negative-control OK: harness reported red as expected` |

The reason is structural, not incidental. `ci/release-parity/run.sh:21` sources exactly
**one** ecosystem module per invocation, and only `release-plz.sh` resolves a binary
through `proto bin`. `python-semantic-release.sh:28` resolves through `uv run`;
`semantic-release.sh` invokes `node` directly. Neither captures proto's stdout.

This correction is part of the deliverable. Leaving the "all three" claim in CLAUDE.md
would keep sending the next reader to look for two failures that do not exist.

## 3. Audit of every `proto bin` capture (AC4)

`grep -rn 'proto bin'` over `ci/`, `.moon/`, `moon.yml` and `.github/` returns **one**
hit: `release-plz.sh:16`. There is no second call site to fix.

`CARGO_BIN` (`release-plz.sh:19`) is **not** affected. It uses `command -v cargo`, which
resolves to `/Users/…/.cargo/bin/cargo` — rustup's proxy, not a proto shim — so no proto
output is ever captured. Measured.

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

`.prototools` pins `proto = "0.61.1"`, so the flag is present for every CI run and every
correctly provisioned checkout. A contributor on an older proto that lacks the flag gets
a non-zero exit, which D2 reports readably.

### D2 — Assert the resolution, at the point of resolution

Immediately after the capture, assert the result is an executable file. On failure,
print the file, the resolved value and the likely cause, and exit **2**.

Exit 2 is not arbitrary: `run.sh` already distinguishes rc=2 (the harness could not run)
from rc=1 (the gate reported red), and SMA-530's negative controls are built on that
distinction. A resolution failure is an infrastructure failure and must keep reporting
as one.

The ecosystem module is `source`d by `run.sh` after its argument parse (`run.sh:12`
parses, `:21` sources), and `-h|--help` exits at `:15` before the source. So an `exit`
from module top level cannot pre-empt argument handling or help output.

### D3 — Drop the fallback chain

`|| command -v release-plz || echo release-plz` is removed.

Neither fallback has ever executed, because `proto bin` exits 0. Reinstating them behind
the fixed primary would be worse than leaving them dead:

- `command -v release-plz` resolves to `~/.proto/shims/release-plz`. The script's own
  header comment (`release-plz.sh:11-15`) records that a shim resolves its version by
  walking up from CWD to find `.prototools`, and that this fails from the fixture
  directory in CI with `proto::tool::unknown_id`. **Measured here: the shim does work
  from `/tmp` on this machine and returns the pinned 0.3.158.** There is no global
  `release-plz` pin in `~/.proto/.prototools` that would explain the difference. So the
  header's claim is unreproduced locally, and this spec does not assert the shim is
  broken — only that its behaviour differs between environments and is therefore not a
  trustworthy fallback for a version-comparison harness.
- `echo release-plz` is not a resolution at all. It defers to `PATH` and hopes.

The harness exists to compare one specific pinned release-plz's classification
behaviour against the other two ecosystems. A silently substituted binary — a different
version, or a shim that resolves differently in CI than locally — produces a parity
verdict about the wrong thing. Failing loudly is the correct behaviour for this file.

Accepted cost: a contributor without proto installed now gets a hard error instead of a
run that might have worked. That is the intended trade. `CONTRIBUTING.md` already makes
`proto install` the first step of local setup.

### D4 — No new `repo:*` gate

Considered and rejected for this issue. A gate scanning `ci/` for an unguarded
`$(proto bin …)` capture would cost the full ritual — the `ci.yml` `T=(…)` array,
CLAUDE.md's marker-delimited command, an `affected-smoke` re-baseline, self-tests and a
negative control — to guard **one** line of code that this change is deleting the
hazardous form of.

The residual is real and is recorded rather than closed: a second `$(proto bin …)`
capture written elsewhere in `ci/` tomorrow would carry the same defect and nothing
would red. See §7.

### D5 — Correct CLAUDE.md rather than delete the entry

CLAUDE.md lines 52-59 carry the NDJSON gotcha with two things now wrong: the "all three
`repo:release-parity*` gates abort" scope claim (§2), and the `unset AI_AGENT CLAUDECODE
CLAUDE_CODE_ENTRYPOINT` workaround, which this change removes the need for.

The entry stays, because the underlying proto behaviour is unchanged and still breaks
any *new* `$(proto bin …)` capture a future author writes. It is rewritten to state the
behaviour, name `--reporter text` as the fix to use, and point at `release-plz.sh` as
the worked example. The cross-reference at line 84 ("The NDJSON entry above is the same
root tool, a different symptom") stays valid and is left alone.

## 5. The change

One functional file. `ci/release-parity/ecosystems/release-plz.sh`, lines 11-16, becomes
roughly:

```bash
# release-plz and the `cargo metadata` it spawns run inside a temp fixture OUTSIDE
# this repo, so resolve the absolute tool binary once, from the repo, and invoke that.
#
# --reporter text is REQUIRED, not cosmetic: proto prints NDJSON on stdout when it
# detects an agent environment (AI_AGENT / CLAUDECODE / CLAUDE_CODE_ENTRYPOINT), and
# `proto bin` exits 0 while doing it — so an unflagged capture silently yields a JSON
# blob instead of a path, and every `||` fallback is skipped. (SMA-596)
_RP_SELF="${BASH_SOURCE[0]:-$0}"
_RP_REPO_ROOT="$(cd "$(dirname "$_RP_SELF")/../../.." && pwd)"
RELEASE_PLZ_BIN="$(cd "$_RP_REPO_ROOT" && proto --reporter text bin release-plz)" || {
  echo "FATAL: ci/release-parity/ecosystems/release-plz.sh: 'proto bin release-plz' failed." >&2
  echo "       Run 'proto install' in the repo root." >&2
  exit 2
}
[ -x "$RELEASE_PLZ_BIN" ] || {
  echo "FATAL: ci/release-parity/ecosystems/release-plz.sh: release-plz resolved to a non-executable value." >&2
  echo "       Got: $RELEASE_PLZ_BIN" >&2
  echo "       If that looks like JSON, proto's agent-mode NDJSON leaked past --reporter text (SMA-596)." >&2
  exit 2
}
```

Exact wording is the plan's business. The contract this spec fixes is: resolve with
`--reporter text`, assert executability, exit 2 with a readable message naming the file,
the value and the likely cause, and no fallbacks.

Documentation changes:

- `CLAUDE.md` — rewrite the NDJSON bullet per D5.
- `ci/release-parity/README.md` — add the D4 residual to its Limitations section.

## 6. Verification

There is no unit-test layer under `ci/release-parity/`; the harness verifies by running.

1. **AC1 / AC2 — before and after, in an agent session, no variables unset by hand.**
   The "before" table in §2 is already recorded. Re-run all three negative controls
   after the change and record the result. `repo:release-parity` must move rc=2 → rc=0
   with `negative-control OK`. The other two must stay rc=0 — a regression there would
   mean the change reached a module it should not have.
2. **The real suites, not only the controls.** Run all three without
   `--negative-control` and confirm they pass. A control proves the harness can report
   red; only the real run proves the parity assertion itself still holds.
3. **AC3 — prove the assertion bites, by mutation.** Force a bad resolution (replace the
   `proto` call with an `echo` of a JSON blob), confirm the failure is the new message
   naming line 16, not `line 103: … No such file or directory`, then restore. Restore by
   reverting the edit, not by moving a `.bak` file back — a backwards mtime makes cargo
   serve a stale artifact and the re-run then fails for an unrelated reason.
4. **Prove the flag is what fixes it.** Remove `--reporter text` alone, with the fix
   otherwise in place, and confirm the new assertion reports the JSON blob. This
   distinguishes "the assertion fires" from "the flag works".
5. **Full graph.** `moon ci` over the documented target list, since `ci/release-parity/**`
   is an input to all three gates and to `repo:affected-smoke`.

## 7. Limitations, stated

- **L1 — a future `$(proto bin …)` capture is still unguarded.** D4's accepted residual.
  Nothing reds if a second call site is written the broken way. The mitigation is the
  rewritten CLAUDE.md entry and this file, both of which name `--reporter text`.
- **L2 — the shim fallback's CI behaviour is recorded, not reproduced.** §D3 relies on
  the file header's `proto::tool::unknown_id` note for CI, which this work did not
  reproduce. The decision to drop the fallback does not depend on that claim being true;
  it rests on the harness needing a known-version binary.
- **L3 — `--reporter text` is verified on proto 0.61.1 only.** The flag's presence on
  earlier proto releases was not measured. `.prototools` pins 0.61.1, so this bounds
  the supported configuration rather than the observed one.
- **L4 — CI remains unproven for this path.** CI sets none of the detection variables,
  so CI exercises the same code path it always did. The fix is verified locally, in an
  agent session, which is exactly the environment that was broken.

## 8. Out of scope

- Any change to `python-semantic-release.sh` or `semantic-release.sh`. §2 measures both
  as unaffected.
- Any change to `CARGO_BIN`. §3 measures it as unaffected.
- A `repo:*` gate for proto invocations. D4.
- SMA-597 (`ciReport.json` carries no stdout/stderr). SMA-597 names this issue as
  sharing a root tool and asks whether they share a fix. They do not: SMA-597's measured
  cause is a proto **shim exec failure** (`EACCES`), and its deliverable is a corrected
  diagnosis procedure plus a plan-template fix. No line of this change affects it.
