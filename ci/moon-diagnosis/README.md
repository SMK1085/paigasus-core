<!-- SPDX-License-Identifier: Apache-2.0 -->
<!-- moon-diagnosis:ok -->

# `repo:moon-diagnosis-exec`

This gate runs the root `CLAUDE.md` procedure "Diagnosing an unattributed `moon ci` failure"
against a real failed `moon ci` report (SMA-714). Check 12 of `repo:actionlint` gates only the
presence of five literals in that block. This gate runs the Step 1 query and checks the Step 2
and Step 2a facts.

Spec: `docs/superpowers/specs/2026-09-28-sma-714-moon-diagnosis-exec-gate-design.md`.

## Why it exists

SMA-597 wrote the Step 1 `jq` query with `{command, exitCode}`. Moon writes these two fields
under `.meta`, so the query printed `null` for both. Every gate stayed green until SMA-711 fixed
the query. Without this gate, an edit can break the query again with no red.

## What it does

1. It reads the text between the two `moon-diagnosis` markers of the root `CLAUDE.md`. It finds
   the `Step 1` heading and the `bash` fence after it, and takes the fence body as the query.
2. It checks the shape of the query: the word `jq`, zero or more of the flags `-r`, `-c`, `-s`,
   `-e`, one single-quoted program, then the path `.moon/cache/ciReport.json` and nothing else.
   The gate never runs doc text through `bash`. It calls `jq` directly with the program.
3. It makes a throwaway git workspace under a `mktemp -d` directory. The workspace pins the moon
   version of the repo `.prototools` and has five tasks: `fail3` (`exit 3`), `multi` (a two-line
   script that ends with `exit 4`), `missing` (a command that does not exist), `slow`
   (`sleep 20` with a 2 s timeout) and `ok` (`true`). Every fixture `git` call ignores the host's
   global and system git config, so the host's commit signing never applies.
4. It runs `moon ci` in that workspace with an allowlist environment (`env -i` and seven named
   variables). An inherited `MOON_WORKSPACE_ROOT` would move the run to the real workspace, and
   an inherited `PROTO_MOON_VERSION` would override the fixture pin. See `ci/CLAUDE.md`, section
   "A nested `moon` call that targets a fixture workspace".
5. It proves that the fixture ran as designed, with its own `jq` check: four `failed` task
   actions and one `passed` action. It also proves that moon used `--base`.
6. It runs the documented query and compares each output row with the expected
   `{status, exitCode, command}` values.
7. It checks the state files of each failed task (Step 2) and that
   `0 <= finishedAt_ms - lastRunTime <= 1000` (Step 2a).

## Modes

- `run.sh`: the real run on the working-tree `CLAUDE.md`.
- `run.sh --self-test`: fixture tables for the pure units, with no moon and no git. Each case
  asserts the exact verdict text, and the table asserts its own case count.
- `run.sh --negative-control`: a copy of `CLAUDE.md` in which the Step 1 query is the pre-SMA-711
  query. The full real path must fail with exactly four `bad-exec <label> object` rows.

## Exit codes

- `0`: the procedure holds.
- `1`: an assertion failed. Each row is printed as `ROW <row>`.
- `2`: an infrastructure error. The message holds `infrastructure error (rc=2)`. Examples: no
  `moon`, `jq`, `git` or `python3`; a fixture moon version that differs from the pin; no fixture
  report; a nested run that changed the real workspace report; a fixture that did not run as
  designed.

## Registration

The gate is registered in the seven places that `ci/CLAUDE.md` ("Registering a new `repo:*`
gate") lists. `MOON_DIAGNOSIS_SH_CALL_SITES` in `ci/affected-graph/ci_targets.py` pins 59 lines of
`run.sh` as whole lines. If you change a pinned line, change the pin in the same commit.

## Limits

- The gate runs only when `CLAUDE.md`, `.prototools` or `ci/moon-diagnosis/**` changes. A moon
  upgrade that arrives by another path does not schedule it.
- It checks Step 1, Step 2 and Step 2a only. Step 0, Step 3, the "What cannot work" paragraph and
  the CI note are prose or commands that change state. No gate runs them.
- It reads only the root `CLAUDE.md` block. Diagnosis advice in other files is not checked
  (`ci/actionlint/README.md` L32).
- The nested run models a LOCAL run, because the procedure is for local runs. The CI-mode
  behaviour of `--base` is not checked.
- `jq` comes from the runner image in CI and from the host locally. It is not pinned.
- The Step 2a bound of 1000 ms rests on macOS and Linux samples with a maximum of 15.6 ms. If a
  false red occurs, measure the difference again before you change the bound.
- The nested `moon ci` can abort with the proto-shim error that `ci/CLAUDE.md` records for
  `repo:affected-smoke`. The gate reports it as rc 2.
