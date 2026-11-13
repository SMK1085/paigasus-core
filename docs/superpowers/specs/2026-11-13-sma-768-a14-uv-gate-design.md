# SMA-768 — A14: replace the uv parser with an allow-list of approved forms

- Linear: SMA-768 (follows SMA-765, PR #457, merged as `8ef62d58`)
- Date: 2026-11-13
- Status: revision 2, after the builder check and the spec challenge (§10). Sven approved it at
  GATE 1 on 2026-11-13. His decisions are in §3 and §8.

## 1. Intent

SMA-765 added check A14 to `repo:affected-smoke` (`check_uv_env_isolation` in
`ci/affected-graph/cargo_moon_parity.py`). A14 reds when a Moon task can change a uv project
environment in place. Six review rounds made A14 a small shell parser: a segmenter
(`_uv_segments`), a word splitter (`_uv_words`, `_uv_tokens`), a quote tracker (`_uv_quoted_at`),
a verb finder over a closed list (`UV_SUBCOMMANDS`, `UV_GLOBAL_VALUED`, `UV_RUN_VALUED`) and three
denylists (`UV_MUTATING_VERBS`, `UV_PIP_MUTATING`, `UV_RUN_MUTATING_FLAGS`). SMA-768 lists six
gaps that this parser leaves open, and asks for one decision:

- Option 1: close the gaps that matter in the current parser.
- Option 2: replace the parser with a smaller, blunter rule: an allow-list of exact approved `uv`
  forms.

This spec measures both options on the real tree and recommends option 2.

## 2. Measured facts

All numbers are from this branch (base `8ef62d58`), measured with `moon query projects` on
moon 2.5.3 and uv 0.11.16. The measurement scripts are throwaway files in the session scratch
directory. They are not committed. The spec challenger checked every line number in M3 against
the tree.

- M1. **Size of A14 today.** The parser and the check are 412 lines
  (`cargo_moon_parity.py:3225-3636`), 346 of them code. The waiver table
  `ALLOW_UV_ENV_SCRIPT` holds 4 entries. The self-test has a 61-row parser table (41 red rows,
  20 green rows) and about 30 check-level rows (`:6865-7070`, 206 lines).
- M2. **Scan scope today.** 171 Moon tasks have a command. A14 reads their blobs and 18 scripts
  (the `ci/**/*.sh` closure through `UV_SOURCE_STMT_RE`, plus `py/scripts/run_tests.sh`). On the
  real tree A14 gives 0 rows.
- M3. **Every executed `uv` form on the real tree.** There are 11 `uv` uses in task blobs and 17
  executed `uv` uses in the scanned scripts. They use 2 verbs and 6 options (`--isolated`,
  `--reinstall-package`, `--locked`, `--frozen`, `--project`, `--python`):

  | Form | Where |
  |---|---|
  | `uv build` (no other word) | `paigasus-{kernel,ml,proto,workflows}-py:build` |
  | `uv run <cmd>` | `py:fmt`, `py:lint`, `py:typecheck` (3), `paigasus-gateway-conformance-py:test`, `py/scripts/run_tests.sh:17,32` (3) |
  | `uv run --isolated --reinstall-package paigasus-py-bindings pytest tests` | `paigasus-kernel-py:test` (the floor task) |
  | `uv run --locked --project <v> [--python '>=3.12'] <cmd>` | `ci/actionlint/run.sh:4619,4655,4669,5305,5982`, `ci/ruff/run.sh:90`, `ci/helm-render/run.sh:93`, `ci/wasm-lockstep/run.sh:52` |
  | `uv run --project "$HERE" --python '>=3.12' <cmd>` | `ci/workflow-credentials/run.sh:33,70,78` |
  | `uv run --frozen <cmd>` | `ci/release-parity/ecosystems/python-semantic-release.sh:48` |
  | `uv lock` | `ci/version-lockstep/run.sh:1635`, inside `run_write`. `repo:version-lockstep` does not pass `--write`, so no Moon task runs this line. |
  | `command -v uv` (a presence probe) | `ci/helm-render/run.sh:92`, `ci/wasm-lockstep/run.sh:51`, `ci/workflow-credentials/run.sh:26` |

  The `--project` values are `py`, `ci/release-plan` and `"$HERE"`. The only `--python` value is
  `'>=3.12'`. Every command word is a bare literal: `pytest`, `python`, `python3`, `ruff`,
  `basedpyright`.
- M4. **The six gaps, probed on today's parser** (`uv_mutations`):

  | Gap | Probe | Today |
  |---|---|---|
  | 1. closed subcommand list | `uv frobnicate --sync` | green (missed) |
  | 2. `:=` default assignment | `echo ${VIRTUAL_ENV:=x}`, `: "${UV_PROJECT_ENVIRONMENT:=.venv-x}"` | green (missed) |
  | 3. assignment with no command | `UV_PROJECT_ENVIRONMENT=.venv-x`, `VIRTUAL_ENV=/x` + newline + `uv run pytest` | **red** (not reproduced, see P3) |
  | 4. a script that is not UTF-8 | `Path.read_text()` raises `UnicodeDecodeError` | traceback, exit 1 (reads as an assertion failure, not rc 2). `UnicodeDecodeError` is a `ValueError`, and `INFRA_ERRORS` (`:77`) does not list it. The first read is in the shared `source` walk (`:1198`), before A14's own read (`:3614`). |
  | 5. `scripts/*.sh` outside `py/` | `git ls-files` | no such `.sh` file exists today. All 18 files that A14 reads are matched by an input of `repo:affected-smoke` today. Nothing reds when a new one is not. |
  | 6. `source` text inside a quoted argument | by design | fail-closed (a false red), not a hidden mutation |

- M5. **More forms that today's parser misses** (found by the same probe):

  | Probe | Today | Option 2 (§4.2) |
  |---|---|---|
  | `uv lock` (writes `py/uv.lock`) | green | red |
  | `uv run --upgrade pytest` (can change versions in place; a named limit today) | green | red |
  | `uv tool install ruff` | green | red |
  | `uv run pip install x` (writes `py/.venv` when pip is in it) | green | green (R3) |
  | `UVB=uv; "$UVB" sync` | green | red (the bare `uv` word is not an approved form) |
  | `UVB=$(command -v uv); "$UVB" sync`, `${x}v sync` | green | green (R2) |
  | `py/.venv/bin/pip install x` | green | green (R4) |

  The SMA-768 issue text says that A14 covers `py/uv.lock`. It does not: `uv lock` is green
  today.
- M6. **Option 2 on the real tree** (a prototype of §4.2, measurement only). Every executed form
  in M3 is green, except `uv lock` at `ci/version-lockstep/run.sh:1635`. 27 physical lines are
  red. 26 of them only name `uv` in a message, a `printf`, a `grep` pattern or Python data
  (for example `die_infra "uv is not on PATH …"` and `ws.get("tool", {}).get("uv", {})`). Of
  today's 4 waivers, 3 are among the 27. The fourth (`python-semantic-release.sh:34`) is a
  full-line comment, which option 2 does not scan, so it goes away. So the waiver table grows
  from 4 to 27 entries: 26 message or data lines and the `uv lock` line.
- M7. **Option 2 on today's 61 parser rows.** All 41 red rows stay red. Of the 20 green rows,
  14 turn red and 6 stay green. The 14 are not an approved form: the six `uv version` rows,
  `uv run --isolated --force-reinstall pytest`, `uv --directory sync run pytest`, four
  `--isolated` rows with an option or package that is not approved, `echo uv run's own status`,
  and `touch a b && uv run --isolated --reinstall-package p pytest tests` (`p` is not
  `paigasus-py-bindings`). None of the 14 forms occurs on the real tree.
- M8. **The real-tree negative control under option 2.** Remove `--isolated` from the kernel
  task. The prototype gives a row for `paigasus-kernel-py:test`
  (`--reinstall-package without --isolated`).
- M9. **Waiver churn.** Each of the 27 lines in M6 was added after 2026-08-01, in exactly one
  commit (`git log -S`), and no later commit changed it. So the measured re-key rate of an
  existing waiver is 0. The measured rate of NEW waivers is the rate at which new message lines
  name `uv`: 27 lines in about 3.5 months. Most came with a new gate script (seven of the eight
  scripts were created in that window).
- M10. **`UV_*` names.** No task `env:` has a key that starts with `UV_`. In the 18 scanned
  scripts, every `UV_` name is in a full-line comment. So a rule that reds any non-comment `UV_`
  name costs 0 waivers today.

## 3. Decision

**Option 2.** Sven chose it at GATE 1 (2026-11-13). The reasons below were the recommendation
that he approved.

Reasons:

1. The real need is small. The real tree uses 2 verbs, 6 options and 4 option values (M3). A
   short allow-list covers all of it, and anything else reds until a person approves it.
2. Option 2 fails closed by design. Gap 1, `uv lock`, `--upgrade`, `uv tool` and every future
   uv verb or flag are red with no extra rule. Option 1 must add each one to a denylist, and a
   form that nobody added stays green (M4, M5).
3. Most of the parser goes away. A small quote tracker stays for the floor (§4.2). The rest of
   the 346 code lines and the denylists go. The six review rounds of SMA-765 were spent on the
   parser's corner cases.

Costs of option 2, stated plainly:

- C1. The waiver table grows from 4 to 27 entries (M6). Each new script line that only prints or
  stores the word `uv` needs a new entry. M9 measured that rate at 27 lines in about 3.5 months.
- C2. 14 synthetic forms that are green today become red (M7). A future task that needs one of
  them must add it to the allow-list, with a self-test row.
- C3. Both options miss `uv` reached through a computed word (R2) and a mutating command that
  `uv run` executes (R3).

Option 1, for comparison: keep the parser. Pin `UV_SUBCOMMANDS` to the uv version in
`.prototools`, so a uv bump reds until a person reviews the list (gap 1). Add a `:=` pattern
(gap 2). Add `lock`, `tool`, `--upgrade` and `--upgrade-package` to the denylists (M5). Gaps 4
and 5 are fixed the same way as in option 2 (§4.3). Estimated cost: about 40 more code lines and
about 12 more rows, on top of the 346-line parser. Each new uv form after that is green until a
person adds it.

## 4. Design (option 2)

### 4.1 What stays

- The scan scope: the task blob (A5's `invocations`), the `ci/**/*.sh` scripts from
  `task_script_refs`, the project `scripts/*.sh` files from `PROJECT_SCRIPT_REF_RE`, and the
  `source` closure of each through `_uv_script_closure` and `UV_SOURCE_STMT_RE`.
- `SOURCE_STMT_RE` and `task_script_closure` do not change. A8 and A10 follow exactly what they
  follow today.
- `ALLOW_UV_ENV_SCRIPT`: keyed by (script path, exact stripped PHYSICAL line), with a non-empty
  reason. An empty reason is a row. A stale entry is a row. Blob lines cannot be waived.
- `REQUIRED_UV_ISOLATED_TASKS = ("paigasus-kernel-py:test",)` and its `FLOOR:` rows.
- The real-tree negative control in `main()`, unchanged in behavior.
- The findings key `a14` and its place in `EXPECTED_FINDING_KEYS`.
- The row format. A per-task row starts with `"{target}: "`. A row for a scanned script reads
  `"{target}: {path}:{line} {reason}"`. The negative control depends on the `{target}` prefix
  (`r.startswith("paigasus-kernel-py:test")`).

### 4.2 The new rule

1. **Comment lines, per physical line.** A physical line whose first non-blank character is `#`
   is a comment. Skip it. A comment line never joins the next line, even when it ends in `\`:
   bash does not continue a comment. A `#` later in a line is not a comment here:
   `x # uv sync` stays a hit (fail-closed).
2. **Logical lines.** A non-comment physical line that ends in `\` joins the next physical line.
   A row names the physical line of the match.
3. **Every `uv` token is checked.** A `uv` token is `UV_TOKEN_RE` (unchanged:
   `(?<![\w.-])uv(?![\w.-])`). Every match in a non-comment line must be an approved form, or
   the line is a row. A match inside a quoted string is checked too: `bash -c "uv sync"` and
   `"$(uv run …)"` execute. Python heredoc data is checked too, so
   `subprocess.run(["uv", "sync"])` reds.
4. **One invocation.** The text after the match ends at the first `&&`, `||`, `;`, `|`, `)` or
   backtick. This cut is not quote-aware, on purpose. Split the rest on whitespace.
5. **Approved forms.** Nothing else is approved.
   - A1. `uv build` with no further word in the invocation.
   - A2. `uv run`, then zero or more approved options, then a command word.
     - Approved flags: `--locked`, `--frozen`, `--isolated`.
     - Approved valued options, with pinned values: `--project` with `py`, `ci/release-plan` or
       `"$HERE"`; `--python` with `'>=3.12'`. The value is the next word, compared as exact text.
       The `--opt=value` form is not approved.
     - `--reinstall-package paigasus-py-bindings` is approved only when `--isolated` is in the
       same option region.
     - The option region ends at the first word that does not start with `-`. That word is the
       command word. It must be a bare literal: `[A-Za-z0-9_][A-Za-z0-9_.+/-]*`. No quote, no
       `$`, no `<`, no `>`, no `\`. A2 with no command word is red. The words after the command
       word are the command's arguments, and they are not checked.
   - A3. `command -v uv`, with only redirection words after it in the invocation. A redirection
     word is `>/dev/null`, `2>/dev/null` or `2>&1`. `command uv sync` (no `-v`) is red.
6. **Environment names.** Any non-comment occurrence of a `UV_[A-Z0-9_]+` name, `VIRTUAL_ENV` or
   `--active` is a row: an assignment, a `:=` default, an export, an unset, or a mention. Each
   name has word boundaries: `(?<![\w])` before and `(?![\w])` after, and `--active` uses
   `(?<![\w-])` and `(?![\w-])`. Measured: 0 such lines on the real tree (M10).
7. **Task `env:`.** A task `env:` key that is `VIRTUAL_ENV` or starts with `UV_` is a row
   (today: only `UV_PROJECT_ENVIRONMENT` and `VIRTUAL_ENV`). Measured: 0 such keys (M10).
8. **`uv lock` waiver guard.** A row when a task blob names `ci/version-lockstep/run.sh`
   together with `--write`. This keeps the reason of the `uv lock` waiver true.
9. Delete the parser and the denylists: `_uv_segments`, `_uv_words`, `_uv_tokens`,
   `_uv_invocations`, `_uv_run_split`, `_uv_option_region`, `_uv_version_changes`, `_uv_hits`,
   `UV_SUBCOMMANDS`, `UV_GLOBAL_VALUED`, `UV_RUN_VALUED`, `UV_VERSION_SAFE_FLAGS`,
   `UV_MUTATING_VERBS`, `UV_PIP_MUTATING`, `UV_RUN_MUTATING_FLAGS`, `UV_ENV_RES`. Keep
   `uv_mutations(text)` as the public name of the per-text check. Keep a small quote tracker
   (today's `_uv_quoted_at`, or an equal one) for the floor only.

The approved sets are module constants, so a new form is one reviewed edit plus a self-test row.
The names are the builder's call.

**The floor.** The floor task passes when one invocation (rule 4) in a non-comment line of its
blob meets all of these:

- the `uv` is the first word of a logical line, or it follows `&&` or `;`, and the quote tracker
  says it is not inside a quote. A wrong answer of the tracker only removes credit, so it fails
  closed;
- its option region has `--isolated` and `--reinstall-package paigasus-py-bindings`, in any
  order;
- its command is `pytest`, `python -m pytest` or `python3 -m pytest`.

**The waivers.** Replace the 4 entries with the 27 lines from M6. Give each entry a reason, and
start the reason with its class: `PROSE` (a message), `DATA` (a pattern or Python data) or
`EXECUTED`. Exactly one entry is `EXECUTED`: the `uv lock` line at
`ci/version-lockstep/run.sh:1635`. Its reason says that the line is in `run_write`, that no Moon
task passes `--write`, and that rule 8 guards this. The builder derives the final list from the
real run. The builder must not add a second `EXECUTED` entry without asking.

### 4.3 Fixes that do not depend on the option

- G4 (UTF-8). Both reads of a scanned script read with `encoding="utf-8"`: the shared read in
  the `source` walk (`:1198`) and A14's own read. Each wraps a `UnicodeDecodeError` in
  `MoonOutputError(f"{path}: not valid UTF-8 …")`, so the gate exits 2 with a message that names
  the file. For A8 and A10 this changes only the error class of such a file, not what they
  follow.
- G5 (reachability). A14 gives a row for each file that it reads but that no input of
  `repo:affected-smoke` matches. Use the same match as A13 (`_moon_glob_re` over the task's
  `inputGlobs`, the `inputFiles` list, negated globs excluded). A missing `repo:affected-smoke`
  task, or one with no `inputGlobs`, is a row, as in A13. It never skips in silence. Measured:
  0 rows on the real tree. The self-test fixture `a14_proj` gets a `repo:affected-smoke` task
  with `ci/**/*` and `py/scripts/**/*`, so today's check-level rows keep their meaning. Whether
  to share one helper with A13 is the builder's call. A13's rows must not change. The `Fix:`
  text names the two places to edit: the `inputs` of `repo:affected-smoke` in `moon.yml`, and
  `T_AFFECTED_SMOKE_REQUIRED_INPUTS` in `ci/actionlint/run.sh`.

### 4.4 Accepted residuals (go into the README A14 entry)

- R1 (gap 6). A `source` word inside a quoted argument is followed. Fail-closed: at worst a false
  red or an infrastructure error. A correct fix needs a full shell parser.
- R2. `uv` reached through a computed word is not seen: `UVB=$(command -v uv); "$UVB" sync`
  (A3 approves the probe), `${x}v sync`, `eval`.
- R3. A mutating command that `uv run` executes (`uv run pip install`,
  `uv run python -m pip install`) is not seen. The command's arguments are not checked.
- R4. A direct write such as `py/.venv/bin/pip install` is not seen.
- R5. The existing limits stay: Moon toolchain actions (`InstallDependencies`) and the inexact
  relock of a plain `uv run` (SMA-765 R-toolchain, R-relock).
- R6. The cut in rule 4 is not quote-aware. A separator inside a quoted value ends the
  invocation early. Every option value is then compared as exact text, so a cut value is red.
- R7. uv configuration is not scanned. `reinstall`, `reinstall-package`, `upgrade`,
  `upgrade-package` or `cache-keys` in `[tool.uv]` or in a `uv.toml` can make an approved plain
  `uv run` change `py/.venv` in place. Rules 6 and 7 close the environment-variable half of this
  class.
- R8. A script that a scanned script RUNS, but does not source, is not scanned. Example:
  `ci/actionlint/run.sh:4643` runs `bash ci/release-plan/run.sh`. This is today's scope too.

## 5. Self-test and proof

### 5.1 Rows

- Keep today's 41 red parser rows, unchanged.
- Keep today's 6 green rows that stay green under option 2, unchanged. One of them is
  `uv run pytest # --reinstall` (`:6905`).
- Keep the 14 rows of M7, with the expectation changed to red and a label that says "not an
  approved form".
- New red rows:
  - gap 1: `uv frobnicate --sync`;
  - gap 2: `echo ${VIRTUAL_ENV:=x}`, `: "${UV_PROJECT_ENVIRONMENT:=.venv-x}"`;
  - gap 3: `UV_PROJECT_ENVIRONMENT=.venv-x` with no command;
  - M5: `uv lock`, `uv run --upgrade pytest`, `uv tool install ruff`, `UVB=uv; "$UVB" sync`;
  - rule 1: `# uv run --isolated \` + newline + `uv sync`; `x # uv sync`;
  - rule 5, command word: `uv run --locked "--reinstall" pytest`,
    `uv run --locked >/dev/null --reinstall pytest`,
    `uv run --locked 2>/dev/null --upgrade pytest`, `uv run --locked $F pytest`;
  - rule 5, values: `uv run --python 3.13 pytest`, `uv run --project --reinstall pytest`,
    `uv run --project "a b" --reinstall pytest`, `uv run --project $(x) --reinstall pytest`,
    `uv run --project=py pytest`;
  - rule 5, other: `uv run --locked` (no command), `uv build --wheel`,
    `bash -c "uv run --locked pytest; uv sync"`,
    `uv run --reinstall-package paigasus-py-bindings pytest`, `command uv sync`;
  - rule 6: `X=--active`, `UV_PYTHON=3.13 uv run pytest`;
  - floor: `echo 'x && uv run --isolated --reinstall-package paigasus-py-bindings pytest tests'`
    + newline + `uv run pytest tests` gives a `FLOOR:` row.
- New green rows, one per form in M3, plus the tails:
  `uv build`; `uv build && echo ok`; `uv run pytest`;
  `uv run --locked --project py python3 -c x`;
  `uv run --locked --project ci/release-plan --python '>=3.12' python3 x`;
  `uv run --project "$HERE" --python '>=3.12' python3 x`; `uv run --frozen python -c x`;
  `command -v uv >/dev/null 2>&1`; `command -v uv >/dev/null 2>&1 || exit 2`;
  the floor form; `touch a b && <floor form>`;
  `uv run --reinstall-package paigasus-py-bindings --isolated pytest tests` (free order);
  `uv run \` + newline + `  --locked pytest` (a continuation); `# uv sync` (a comment);
  `cat py/uv.lock`.
- Keep all check-level rows of today (task `env:`, followed `ci/` script, project script,
  `./` and `../` refs, the `source` closure rows, waiver reason and stale rows, the unreadable
  file rows, the floor rows, the quoted-floor row, the jq `. as $rows` row).
- New check-level rows:
  - rule 7: a task `env:` with `UV_PYTHON` is a row.
  - rule 8: a blob `bash ci/version-lockstep/run.sh --write` is a row.
  - waiver key: a waived line that ends in ` \` matches by its stripped physical text.
  - G4: a scanned script with invalid UTF-8 raises `MoonOutputError` that names the file.
  - G5: a scanned script that no input matches is a row; a negated glob that excludes it is a
    row; a projects dict with no `repo:affected-smoke` task is a row.

### 5.2 Mutations

Each mutation must compile and run. Each must make at least the named row fail. The builder
records the results in the PR body.

| # | Mutation | Row that must fail |
|---|---|---|
| 1 | Skip the comment test (rule 1) | `# uv sync` green row |
| 2 | Let a comment line join the next line | `# uv run --isolated \` + `uv sync` |
| 3 | Do not join continuations (rule 2) | the continuation green row |
| 4 | Remove the cut (rule 4) | `uv build && echo ok`; `command -v uv … \|\| exit 2` |
| 5 | Approve any verb | `uv frobnicate --sync`; `uv lock` |
| 6 | Approve any option | `uv run --upgrade pytest` |
| 7 | Accept any value for an approved valued option | `uv run --python 3.13 pytest` |
| 8 | Accept a quoted or `$` command word | `uv run --locked "--reinstall" pytest`; `… $F pytest` |
| 9 | Accept `<` or `>` in the command word | `uv run --locked >/dev/null --reinstall pytest` |
| 10 | Drop "reinstall needs `--isolated`" | `uv run --reinstall-package paigasus-py-bindings pytest` |
| 11 | Accept A2 with no command word | `uv run --locked` |
| 12 | Accept words after A1 | `uv build --wheel` |
| 13 | Accept `command uv` without `-v` | `command uv sync` |
| 14 | Remove the env-name rule (rule 6) | gap 2 and gap 3 rows; `X=--active` |
| 15 | Narrow rule 6 to the two old names | `UV_PYTHON=3.13 uv run pytest` |
| 16 | Narrow rule 7 to the two old names | the `UV_PYTHON` task `env:` row |
| 17 | Remove rule 8 | the `--write` blob row |
| 18 | Floor: drop the quote tracker | the `echo 'x && uv run …'` floor row |
| 19 | Floor: require a fixed option order | the free-order green row |
| 20 | Remove G4's wrap | the invalid-UTF-8 row |
| 21 | Remove G5 | the three G5 rows |
| 22 | Waiver: ignore the reason | the empty-reason row |
| 23 | Waiver: ignore stale entries | the stale-entry row |

Also prove the real-tree negative control against the rule itself: approve
`--reinstall-package` without `--isolated` (mutation 10), run the parity script on the real
tree, and show that `main()` exits 2 with the A14 negative-control message.

### 5.3 Real tree

- `python3 ci/affected-graph/cargo_moon_parity.py --self-test` passes.
- `/bin/bash ci/affected-graph/run.sh --negative-control` and `/bin/bash ci/affected-graph/run.sh`
  pass. Use system bash 3.2 on this Mac. Homebrew bash 5.3 deadlocks on a here-string over
  512 bytes.
- A14 gives 0 rows on the real tree, with the new waiver table.
- The real-tree negative control still exits 2 when the kernel task loses `--isolated`. Prove it
  once by hand: remove the flag in a scratch copy and run the parity script.
- `repo:ruff-ci` passes on the changed Python file. It needs bash 4+ locally.

## 6. Documentation

- `ci/affected-graph/README.md`, A14 entry: rewrite it for the allow-list rule (§4.2), G4, G5
  and the residuals R1-R8. Record each of the six SMA-768 gaps as closed or as a residual, with
  its number.
- The `Fix:` text of `a14` in `collect_findings`: say "use an approved form, or add the form to
  the allow-list with a self-test row; a line that only prints `uv` needs an
  `ALLOW_UV_ENV_SCRIPT` entry". For a G5 row, name `moon.yml` and
  `T_AFFECTED_SMOKE_REQUIRED_INPUTS`.
- Do not change the root `CLAUDE.md` or `ci/CLAUDE.md`.

## 7. Non-goals

- No change to any Moon task, `moon.yml` file or gate script. The tree is already green.
- No `--locked` on the plain `uv run` tasks (SMA-765 N2).
- No change to `SOURCE_STMT_RE`, `task_script_closure`, A8 or A10 (except the G4 error class).
- No full shell parser. No scan of `[tool.uv]` or `uv.toml` (R7).

Anti-goal: an allow-list that is wide enough to pass the 14 synthetic forms of M7. That brings
the parser back under a new name.

## 8. Decisions at GATE 1 (Sven, 2026-11-13)

- P1. **Option 2, the allow-list** (§3). Option 1 is not built.
- P2. **The 27 waivers are accepted** (C1, M6, M9), with the classes `PROSE`, `DATA` and
  `EXECUTED`, keyed by physical line. The re-key rate of an existing waiver is 0, but each new
  message line that names `uv` needs a new waiver.
- P3. **Gap 3 is closed as invalid.** Today's parser already reds the probes in M4. A self-test
  row keeps the form red under the allow-list too (rule 6, the gap-3 row in §5.1).
- P4. **`uv lock` is in scope.** The one real call at `ci/version-lockstep/run.sh:1635` gets an
  `EXECUTED` waiver. Rule 8 reds a Moon task that runs `ci/version-lockstep/run.sh` with
  `--write`. `run_write` does not move.
- P5. **The option values are pinned.** `--project` and `--python` accept only today's 4 values.
  A new value (a new uv project under `ci/`) needs an allow-list edit. Not measured: whether
  `uv run --python <other>` rewrites `py/.venv` on uv 0.11.16. The pin makes that question moot.

## 9. Risks and rollback

- Risk: a new waiver for each new message line that names `uv` (C1). The A14 message names the
  file, the line and the fix.
- Risk: a real new uv need reds A14 (C2, P5). That is the intended behavior.
- Rollback: revert the commit. The real tree does not depend on A14's internals.

## 10. Changelog of revision 2

From the builder check and the spec challenge (verdict: APPROVE WITH CHANGES).

Folded in:

- BLOCKER, a comment line that ends in `\` hid the next line: rule 1 is now per physical line,
  and a comment never joins. New red row and mutation 2.
- BLOCKER, a quoted flag, a redirection or an expansion could be the command word: the command
  word is now a bare literal (rule 5). Four new red rows, mutations 8 and 9.
- MAJOR, valued options took any value: the values are pinned (rule 5, P5).
- MAJOR, no mutation list: §5.2 now lists 23 mutations, each with the row that must fail. New
  green tail rows for the cut, a new floor quote row, a free-order floor row, `X=--active`.
- MAJOR, uv configuration was not a named residual: added R7. Rules 6 and 7 now red every
  `UV_*` name and task `env:` key (0 cost, M10).
- MINOR: M7 counts corrected (41 red, 20 green; 6 stay green and are kept); M3 says 6 options;
  R2 names the exact forms; G4 wraps both reads and names the file; G5 fixture, missing-task
  row and `Fix:` text; the waiver key is the physical line; the row prefix is pinned; rule 8
  guards the `uv lock` waiver; waiver classes with exactly one `EXECUTED`; R8 for scripts that
  are run but not sourced; the `command uv sync` row; a mutation proof of the negative control;
  M9 re-measured on the waived lines only; the quote tracker stays for the floor.
- Builder questions: G5 with no smoke task (a row, and the fixture gets the task); A1 accepts no
  redirection (my call, fail-closed; none on the tree); the floor quote tracker stays; rule 6
  word boundaries are stated.

Not folded in:

- "Measure whether `uv run --python 3.13` replaces `py/.venv`": not measured. The pinned value
  makes the answer irrelevant to the gate (P5).
