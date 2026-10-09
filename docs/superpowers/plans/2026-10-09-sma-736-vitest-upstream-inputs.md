# SMA-736 A13 vitest upstream inputs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add assertion A13 to `repo:affected-smoke`. A13 reds when a ts task that runs vitest does not declare, as Moon `inputs`, a file that vitest reads outside the task's own package. Then fix the real `moon.yml` inputs and deps until A13 passes, and re-baseline `ci/affected-graph/run.sh`.

**Architecture:** A13 lives in `ci/affected-graph/cargo_moon_parity.py`, next to A12. It derives the vitest tasks from `moon query projects` (`VITEST_TOKEN_RE`). It finds each task's config files from the invocation. It parses each config with a narrow single-pass scanner. It reuses A12's `package.json` closure walk, and it shares the workspace-package rule with A12a through a new helper `workspace_package_inputs`. It asks git for the tracked set through `task_inputs.tracked_files`. It asserts containment per task over both input buckets, as A7 and A12a do. Three floors stop it from passing vacuously.

**Tech Stack:** Python 3.11+ (standard library only), Moon 2.5.3 (`moon query projects`, `moon query tasks`), git, bash 3.2 for `ci/affected-graph/run.sh`, Ruff through `uv run --locked --project py`.

**Spec:** `docs/superpowers/specs/2026-10-09-sma-736-vitest-upstream-inputs-design.md` (revision 2, approved 2026-10-09).

## Global Constraints

- Work only in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-736`, branch `feature/sma-736-vitest-upstream-inputs`. Run every command from the worktree root.
- Every source file opens with an SPDX header: `# SPDX-License-Identifier: Apache-2.0` (Python, shell, YAML), `// SPDX-License-Identifier: Apache-2.0` (TS). This plan adds no new source file; keep the headers that exist.
- Python is 3.11 or later (the gate imports `tomllib`). `python3` is `/opt/homebrew/bin/python3`. Never use `/usr/bin/python3` (3.9, no `tomllib`).
- Start each shell with the preamble below: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` and `PROTO_REPORTER=text`. Shell state does not persist between tool calls, so run the preamble again at the start of each command block that needs `moon`, `uv` or `$SCRATCH`.
- Do not pipe a command whose exit status you read into an early-exit reader (`head`, `grep -q`). Write the output to a file under `$SCRATCH` and read the file (ci/CLAUDE.md, SMA-647).
- `ci/affected-graph/run.sh` needs system bash 3.2 on the development Mac. Run it as `/bin/bash ci/affected-graph/run.sh`. Run `moon run repo:affected-smoke` with the bash-only shim directory first on `PATH`. Do not put `/bin` first on `PATH`.
- Export `PROTO_REPORTER=text` before any command that captures `moon` or `proto` output (SMA-609).
- `repo:ruff-ci` lints `ci/**/*.py` with `py/pyproject.toml`: line-length 200, rules `E F W I N UP B A C4 SIM TCH RUF`. Run the Ruff step of each Python task.
- Commits: conventional commits with a workspace scope (`refactor(ci)`, `test(ci)`, `feat(ci)`, `fix(ts)`, `docs(ci)`), `SMA-736` in the subject, and the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Never use `--no-verify`. If `commit-msg` fails with `commitlint not found`, run `pnpm -C ts install`. If a commit fails with `failed to fill whole buffer`, 1Password is locked: stop and ask the coordinator.
- No line in a commit body starts with `#` followed by digits (commitlint `footer-leading-blank`).
- Never use bare `git stash`. Never `git commit --amend`. Never `git reset` to drop a commit.
- New documents and comments do not name the moon CI report file (the JSON report under `.moon/cache/`). `repo:actionlint` check 12 reds a file that names it without a marker.
- Foreground commands only. Do not install host software (`brew`, `npm -g`, `pip install`).
- Use `--force` on every `moon run` whose result is evidence. A cache hit replays an old log.
- Write all prose (comments, docs, commit bodies) in ASD-STE100 Simplified Technical English.
- Expected red windows: from Task 5 to Task 7, `python3 ci/affected-graph/cargo_moon_parity.py` (the real run) exits 1 with `a13` rows. From Task 5 to Task 8, `/bin/bash ci/affected-graph/run.sh` exits 1. That is expected. Each task proves its own work with the commands in its steps.
- In Task 8, if a row looks wrong rather than missing, stop and ask the coordinator. Do not add an input to silence a row that you think is wrong.
- If `git status` shows `ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts` deleted after a run, a BSR rate limit hit `contracts:generate`. Restore it with `git checkout -- ts/packages/paigasus-proto/src/generated`.

### Shell preamble

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma736}"
mkdir -p "$SCRATCH/bashshim" && ln -sf /bin/bash "$SCRATCH/bashshim/bash"
```

### Ruff command (used by every Python task)

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
uv run --locked --project py ruff check --config py/pyproject.toml -- ci/affected-graph/cargo_moon_parity.py ci/affected-graph/task_inputs.py
```

Expected: `All checks passed!`

## Review Focus

These five input classes are the most likely to break the design, and the least likely to be caught by a happy-path test. Most likely first. Each one has a test in its owning task.

1. **A `cd` inside a subshell before the vitest call.** The real `paigasus-kernel-ts:test` runs `( cd ../../../rs/crates/bindings/paigasus-wasm && wasm-pack build … ) && pnpm exec vitest run`. Spec §4.2 says that a `cd` before the vitest call is a row. Read literally, that reds the kernel task with a row that no input can fix. The `cd` is inside a closed `( … )` group, so it does not move vitest. A13 removes closed groups before it looks for `cd`. A `cd` in the same shell must still be a row. Test: Task 3, the lookup rows `cd sub && …` (row) and `touch a && ( cd ../x && … ) && …` (no row); Task 5, row T1 (the fixture kernel task has the real shape).
2. **Scanner states that overlap.** A quote inside a comment, a `//` inside a string, an escaped backtick and a `${ {a: 1}.a }` with nested braces inside a template literal, and the regex literal `/\.node$/` from the real kernel config. A state error loses or invents an alias. This is the SMA-639 `stripComments` defect class. Test: Task 2, the parser table; Task 6, the real-corpus pin (the ui config has a template literal with escaped backticks and a block comment with a quote).
3. **A tsconfig.json with comments.** The real `ts/packages/paigasus-kernel/tsconfig.json` and `ts/packages/paigasus-ui/tsconfig.json` hold `//` comments, so a plain `json.loads` fails. A13 removes the comments with the same scanner first. Test: Task 5, the fixture kernel `tsconfig.json` is JSONC; rows T1 and T2 (`k-ts:test inputs omit ts/packages/kernel/tsconfig.json`).
4. **A task with no config file.** `paigasus-proto-ts:test` and `commitlint-config-ts:test` run vitest with defaults. "Every config sets `tsconfig: false`" is true for an empty set, so a naive `all()` skips their tsconfig files. vitest with defaults loads them. A13 skips tsconfig files only when there is at least one config and every config sets `tsconfig: false`. Test: Task 5, row T9 "a task with NO config file still reads its tsconfig"; also the "one of two configs" row.
5. **The other module's exception class.** `task_inputs.tracked_files` raises `task_inputs.MoonOutputError`, not this file's `MoonOutputError`. If `INFRA_ERRORS` does not hold it, a failed `git ls-files` exits 1 with a traceback, which reads as an assertion failure, not rc 2. Also, a git hook sets `GIT_DIR` and `GIT_INDEX_FILE`; an inherited value makes `ls-files` read another index. Test: Task 4, the two tracked-file rows.

## File structure

| File | Change | Task |
|---|---|---|
| `ci/affected-graph/cargo_moon_parity.py` | New helper `workspace_package_inputs` before `tsc_required_inputs` (line 2293); `tsc_required_inputs` calls it | 1 |
| `ci/affected-graph/cargo_moon_parity.py` | A13 parser block (`_scan_js`, `vitest_config_facts`) before `def moon_projects():` (line 2463) | 2 |
| `ci/affected-graph/cargo_moon_parity.py` | A13 selection block (`VITEST_TOKEN_RE`, `derive_vitest_tasks`, `vitest_invocation_configs`) before `def moon_projects():` | 3 |
| `ci/affected-graph/cargo_moon_parity.py`, `ci/affected-graph/task_inputs.py` | `import task_inputs`; `INFRA_ERRORS` (lines 68-74); `task_inputs._git` clears `GIT_DIR`/`GIT_INDEX_FILE` (lines 253-273) | 4 |
| `ci/affected-graph/cargo_moon_parity.py` | A13 check block (`vitest_required_inputs`, `check_ts_vitest_inputs`, floors) before `def moon_projects():`; registration: header (lines 1-37), `EXPECTED_FINDING_KEYS` (line 5072), `collect_findings` (line 5075), `main` (line 5236), arity call (line 2747), OK message (line 5055) | 5 |
| `ci/affected-graph/cargo_moon_parity.py` | Real-corpus pin rows in `self_test()` | 6 |
| `moon.yml`, `ci/actionlint/run.sh` | `repo:affected-smoke` inputs (after line 217); `T_AFFECTED_SMOKE_REQUIRED_INPUTS` (lines 2127-2177) and its count comment (line 2117) | 7 |
| `.moon/tasks/typescript-project.yml`, `ts/packages/{paigasus-app-shell,paigasus-auth,paigasus-console-core,paigasus-discovery,paigasus-sdk}/moon.yml`, `ts/apps/{iam-console,gateway-console}/moon.yml` | vitest task inputs and one dep | 8 |
| `ci/affected-graph/run.sh` | Rename `napi-glue-js->kernel-test` to `napi-glue-js->vitest` (line 769); re-measured CSVs | 9 |
| `ci/affected-graph/README.md`, `ci/CLAUDE.md` | A13 bullet (after line 316); replace lines 312-313; extend the rule at lines 64-67 | 10 |
| this plan | The Measurements section at the end | 5, 6, 9 |

All new self-test rows go immediately before the two lines at the end of `self_test()` (line 5050 before Task 1):

```python
    for f in failures:
        print(f"  FAIL {f}", file=sys.stderr)
```

Each task adds its rows after the rows of the task before it. All new A13 code goes immediately before the line `def moon_projects():`, after the code of the task before it. Leave two blank lines after each inserted top-level block, as PEP 8 and the file do. Line numbers in this plan are the numbers before Task 1; they move as tasks add lines. Use the quoted anchor text, not the number.

---

### Task 1: Extract the shared helper `workspace_package_inputs` (A12 unchanged)

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` — insert before `def tsc_required_inputs(root, closure, rows):` (line 2293); replace the `if kind == "workspace":` block inside it (lines 2305-2316); self-test rows before `    for f in failures:` (line 5050).

**Interfaces:**
- Consumes: `_export_leaves(node, where, rows) -> list[str]` (line 2275).
- Produces: `workspace_package_inputs(pkg_dir: str, data: dict, rows: list[str]) -> set[str]`.

- [ ] **Step 1: Write the failing self-test rows.** Insert this block immediately before `    for f in failures:` at the end of `self_test()`:

```python
    # SMA-736 T12 — the A12/A13 shared helper, called directly. A12a's rows above already pass
    # through it; this row pins the helper's own contract, so a change to it reds here first.
    rows = []
    got = workspace_package_inputs("ts/packages/core", {
        "exports": {
            ".": "./src/index.ts",
            "./testing": "./testing/index.ts",
            "./preset": {"import": "./preset.json", "require": None},
        },
    }, rows)
    want = {
        "ts/packages/core/package.json", "ts/packages/core/src/**/*",
        "ts/packages/core/testing/**/*", "ts/packages/core/preset.json",
    }
    if got != want or rows:
        failures.append(f"workspace_package_inputs gave {sorted(got)} and rows {rows}, expected {sorted(want)} and no rows")
    rows = []
    workspace_package_inputs("ts/packages/core", {"exports": {".": "src/index.ts"}}, rows)
    if rows != ["ts/packages/core/package.json: the `exports` target 'src/index.ts' does not start with `./`"]:
        failures.append(f"workspace_package_inputs accepted an `exports` target without `./`: {rows}")
```

- [ ] **Step 2: Run the self-test and see it fail.**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "rc=$?"
```

Expected: a traceback that ends with `NameError: name 'workspace_package_inputs' is not defined`, and `rc=1`.

- [ ] **Step 3: Add the helper.** Insert this block immediately before the line `def tsc_required_inputs(root, closure, rows):`:

```python
def workspace_package_inputs(pkg_dir, data, rows):
    """The inputs one workspace package `pkg_dir` adds to a task that reads it through its closure.

    `pkg_dir/package.json`, `pkg_dir/src/**/*`, and each `exports` target outside `src/`:
    `pkg_dir/<head>/**/*` for a target in a subdirectory, `pkg_dir/<file>` for a file at the package
    root. `data` is the parsed package.json. A12a (`tsc`) and A13 (vitest) both call this, so the two
    rules cannot drift (SMA-736 spec §4.3 item 3). An `exports` shape it cannot read is a row.
    """
    want = {f"{pkg_dir}/package.json", f"{pkg_dir}/src/**/*"}
    for leaf in _export_leaves(data.get("exports"), f"{pkg_dir}/package.json", rows):
        if not leaf.startswith("./"):
            rows.append(f"{pkg_dir}/package.json: the `exports` target {leaf!r} does not start with `./`")
            continue
        rel = leaf[2:]
        if rel.startswith("src/"):
            continue
        head, sep, _ = rel.partition("/")
        want.add(f"{pkg_dir}/{head}/**/*" if sep else f"{pkg_dir}/{rel}")
    return want
```

- [ ] **Step 4: Make `tsc_required_inputs` call the helper.** In `tsc_required_inputs`, replace:

```python
        if kind == "workspace":
            want.add(f"{pkg_dir}/src/**/*")
            for leaf in _export_leaves(data.get("exports"), f"{pkg_dir}/package.json", rows):
                if not leaf.startswith("./"):
                    rows.append(f"{pkg_dir}/package.json: the `exports` target {leaf!r} does not start with `./`")
                    continue
                rel = leaf[2:]
                if rel.startswith("src/"):
                    continue
                head, sep, _ = rel.partition("/")
                want.add(f"{pkg_dir}/{head}/**/*" if sep else f"{pkg_dir}/{rel}")
            continue
```

with:

```python
        if kind == "workspace":
            want |= workspace_package_inputs(pkg_dir, data, rows)
            continue
```

- [ ] **Step 5: Run the self-test and see it pass.**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "rc=$?"
```

Expected: the last line is `  OK   [parity] all twelve assertions fire on synthetic violations` and `rc=0`. This is the first half of spec T12: every A12a row still passes after the move.

- [ ] **Step 6: Run Ruff** (the command in Global Constraints). Expected: `All checks passed!`

- [ ] **Step 7: Commit.**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -F- <<'EOF'
refactor(ci): share the A12 workspace-package input rule through a helper (SMA-736)

A12a and the coming A13 need the same rule for a workspace package in a
closure: package.json, src/**/*, and each exports target outside src/.
workspace_package_inputs holds it once, so the two rules cannot drift.
A12a's behaviour does not change.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

### Task 2: The config parser `vitest_config_facts` and its scanner (spec §4.4, T5)

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` — insert the A13 parser block before `def moon_projects():` (line 2463 before Task 1); self-test rows before `    for f in failures:`.

**Interfaces:**
- Consumes: module imports `os`, `re`.
- Produces: `_scan_js(text: str, where: str, rows: list[str]) -> tuple[str, str, dict[int, tuple[int, str]]] | None`; `vitest_config_facts(text: str, config_rel: str, rows: list[str]) -> tuple[list[tuple[str, str]], bool]`; private helpers `_matching_close`, `_top_level_entries`, `_js_bindings`, `_alias_value`.

- [ ] **Step 1: Write the failing self-test rows.** Insert this block before `    for f in failures:` (after the Task 1 rows):

```python
    # A13 (SMA-736) — the config parser, called directly (spec T5). Each row is (label, config text,
    # expected aliases or None to skip the check, expected tsconfig_off or None, the one expected row
    # or None for "no rows at all"). The comment, string and template rows are the SMA-639 defect
    # class: a scanner that opens a string inside a comment, or a comment inside a string, loses or
    # invents an alias.
    a13_cfg = "ts/packages/k/vitest.config.ts"
    a13_ok_alias = [("a", "ts/packages/k/a")]
    for label, text, want_aliases, want_off, want_row in (
        ("an `alias` array", "export default { resolve: { alias: [{ find: 'a', replacement: './a' }] } };\n",
         None, None, f"{a13_cfg}: an `alias` value is not an object literal, so A13 cannot read it"),
        ("a shorthand `alias`", "const alias = { a: './a' };\nexport default { resolve: { alias } };\n",
         None, None, f"{a13_cfg}: `alias` is used without a `:` value (a shorthand or a variable), which A13 cannot read"),
        ("a quoted 'alias' key", "export default { resolve: { 'alias': { '@paigasus/x': '../x/src/index.ts' } } };\n",
         [("@paigasus/x", "ts/packages/x/src/index.ts")], False, None),
        ("a call as a @paigasus/ value", "export default { resolve: { alias: { '@paigasus/x': path.resolve('x') } } };\n",
         None, None, f"{a13_cfg}: the alias @paigasus/x has a value A13 cannot resolve"),
        ("a package name as a @paigasus/ value", "export default { resolve: { alias: { '@paigasus/x': 'other-pkg' } } };\n",
         None, None, f"{a13_cfg}: the alias @paigasus/x has a value A13 cannot resolve"),
        ("an unbound identifier value", "export default { resolve: { alias: { 'server-only': stub } } };\n",
         None, None, f"{a13_cfg}: the alias server-only has a value A13 cannot resolve"),
        ("a package name as a plain value", "export default { resolve: { alias: { react: 'preact/compat' } } };\n",
         [], False, None),
        ("a spread entry", "export default { resolve: { alias: { ...base } } };\n",
         None, None, f"{a13_cfg}: an `alias` object holds a spread entry, which A13 cannot read"),
        ("a computed key", "export default { resolve: { alias: { [k]: './a' } } };\n",
         None, None, f"{a13_cfg}: an `alias` object holds a computed key, which A13 cannot read"),
        ("a trailing comma", "export default { resolve: { alias: { a: './a', } } };\n", a13_ok_alias, False, None),
        ("a `//` comment", "// alias: { '@paigasus/x': stub }\nexport default {};\n", [], False, None),
        ("a `/* */` comment", "/* alias: { '@paigasus/x': stub } */\nexport default {};\n", [], False, None),
        ("`//` inside a string", "const u = 'http://x'; export default { resolve: { alias: { a: './a' } } };\n",
         a13_ok_alias, False, None),
        ("quotes inside comments", "// it's \"a\" `b`\n/* it's \"a\" `b` */\nexport default { resolve: { alias: { a: './a' } } };\n",
         a13_ok_alias, False, None),
        ("a template literal",
         "const m = `x \\` ${ {a: 1}.a } alias: { '@paigasus/y': 1 }`;\nexport default { resolve: { alias: { a: './a' } } };\n",
         a13_ok_alias, False, None),
        ("a regex literal", "export default { test: { server: { deps: { external: [/\\.node$/] } } }, resolve: { alias: { a: './a' } } };\n",
         a13_ok_alias, False, None),
        ("a relative import", "import base from './base.config';\nexport default base;\n",
         None, None, f"{a13_cfg}: imports './base.config'; A13 cannot read a config that is split across files"),
        ("mergeConfig", "import { mergeConfig } from 'vitest/config';\nexport default {};\n",
         None, None, f"{a13_cfg}: uses `mergeConfig`; A13 cannot read a config that is split across files"),
        ("a string `projects` entry", "export default { test: { projects: ['packages/*'] } };\n",
         None, None, f"{a13_cfg}: `projects` holds a string entry; vitest loads it as another config or a glob, which A13 does not follow"),
        ("object `projects` entries", "export default { test: { projects: [{ test: { include: ['a.test.ts'] } }] } };\n", [], False, None),
        ("`tsconfig: false`", "const oxc = { tsconfig: false } as const;\nexport default { oxc };\n", [], True, None),
        ("`tsconfig: false` in a comment", "// tsconfig: false\nexport default {};\n", [], False, None),
        ("a fileURLToPath binding",
         "const d = fileURLToPath(new URL('../../../rs/crates/bindings/nb/index.js', import.meta.url));\n"
         "export default { resolve: { alias: { '@paigasus/node-bindings': d } } };\n",
         [("@paigasus/node-bindings", "rs/crates/bindings/nb/index.js")], False, None),
        ("an unclosed string", "const s = 'abc\nexport default {};\n",
         None, None, f"{a13_cfg}: a ' string is not closed on its line, so A13 cannot read the file"),
    ):
        rows = []
        aliases, off = vitest_config_facts(text, a13_cfg, rows)
        if want_row is None and rows:
            failures.append(f"A13 parser, {label}: unexpected rows {rows}")
        if want_row is not None and want_row not in rows:
            failures.append(f"A13 parser, {label}: expected the row {want_row!r}, got {rows}")
        if want_aliases is not None and aliases != want_aliases:
            failures.append(f"A13 parser, {label}: aliases {aliases}, expected {want_aliases}")
        if want_off is not None and off is not want_off:
            failures.append(f"A13 parser, {label}: tsconfig_off is {off}, expected {want_off}")
```

- [ ] **Step 2: Run the self-test and see it fail.**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "rc=$?"
```

Expected: a traceback that ends with `NameError: name 'vitest_config_facts' is not defined`, and `rc=1`.

- [ ] **Step 3: Add the parser.** Insert this block immediately before the line `def moon_projects():`:

```python
# SMA-736 — A13. Every ts task that runs vitest must key on what vitest reads: its config files, the
# package.json closure that A12 walks, the runtime files of each `file:` binding, the tsconfig.json
# `extends` chains, and each alias target outside its own package. See
# docs/superpowers/specs/2026-10-09-sma-736-vitest-upstream-inputs-design.md.
#
# The config parser below is deliberately narrow (spec §4.4). It reads the alias forms the real
# configs use. Any other form that can carry an alias is a row, never a skip.
_JS_IDENT_RE = re.compile(r"[A-Za-z_$][\w$]*")
_JS_IMPORT_RE = re.compile(r"(?<![\w$.])(?:from|import)\s*\(?\s*(?=['\"])")
_JS_SPLIT_CONFIG_RE = re.compile(r"(?<![\w$])(mergeConfig|extends)(?![\w$])")
_JS_CONST_STRING_RE = re.compile(r"(?<![\w$])const\s+([A-Za-z_$][\w$]*)\s*=\s*(?=['\"])")
_JS_CONST_URL_RE = re.compile(r"(?<![\w$])const\s+([A-Za-z_$][\w$]*)\s*=\s*fileURLToPath\(\s*new\s+URL\(\s*(?=['\"])")
_JS_URL_TAIL_RE = re.compile(r"\s*,\s*import\.meta\.url\s*\)\s*\)")
_JS_STATEMENT_END_RE = re.compile(r"[ \t]*(?:;|\n|$)")
_JS_ALIAS_WORD_RE = re.compile(r"(?<![\w$])alias(?![\w$])")
_JS_KEY_COLON_RE = re.compile(r"\s*:")
_JS_COLON_RE = re.compile(r"\s*:\s*")
_JS_TSCONFIG_OFF_RE = re.compile(r"(?<![\w$])tsconfig\s*:\s*false(?![\w$])")
_JS_PROJECTS_RE = re.compile(r"(?<![\w$])projects\s*:\s*")
_JS_OPEN = "([{"
_JS_CLOSE = ")]}"


def _scan_js(text, where, rows):
    """One pass over a JS, TS or JSONC text. Return (code, masked, literals), or None after a row.

    The scanner has one state at a time: code, `'...'`, `"..."`, a template literal, `// ...` and
    `/* ... */`. A comment opens only in code state, so a quote inside a comment opens no string and
    a `//` inside a string opens no comment. A template literal's `${...}` returns to code state until
    its matching `}`; a stack holds the nesting. This is the defect class ts/CLAUDE.md records for
    `stripComments` (SMA-639), so the self-test covers each state.

    `code` is `text` with each comment replaced by spaces (newlines kept), so every offset stays the
    same. `masked` is `code` with the contents of each string and template literal replaced by `_`
    (quotes kept), so a regex over `masked` never matches inside a string. `literals` maps the offset
    of each `'` or `"` opening quote to (end, value): `end` is the offset after the closing quote.

    Limit: a regex literal is not a state. `/\\.node$/` reads as code and is harmless; a regex that
    holds a quote opens a string and fails closed with a row.
    """
    code = list(text)
    masked = list(text)
    literals = {}
    stack = [["code", 0]]
    i, n = 0, len(text)
    while i < n:
        mode = stack[-1]
        ch = text[i]
        nxt = text[i + 1] if i + 1 < n else ""
        if mode[0] == "tpl":
            if ch == "\\":
                for k in (i, i + 1):
                    if k < n and text[k] != "\n":
                        masked[k] = "_"
                i += 2
            elif ch == "`":
                stack.pop()
                i += 1
            elif ch == "$" and nxt == "{":
                stack.append(["code", 0])
                i += 2
            else:
                if ch != "\n":
                    masked[i] = "_"
                i += 1
            continue
        if ch == "/" and nxt == "/":
            end = text.find("\n", i)
            end = n if end == -1 else end
            for k in range(i, end):
                code[k] = masked[k] = " "
            i = end
            continue
        if ch == "/" and nxt == "*":
            end = text.find("*/", i + 2)
            if end == -1:
                rows.append(f"{where}: a `/*` comment is not closed, so A13 cannot read the file")
                return None
            for k in range(i, end + 2):
                if text[k] != "\n":
                    code[k] = masked[k] = " "
            i = end + 2
            continue
        if ch in "'\"":
            j, value = i + 1, []
            while j < n and text[j] not in (ch, "\n"):
                if text[j] == "\\" and j + 1 < n:
                    value.append(text[j + 1])
                    j += 2
                    continue
                value.append(text[j])
                j += 1
            if j >= n or text[j] != ch:
                rows.append(f"{where}: a {ch} string is not closed on its line, so A13 cannot read the file")
                return None
            for k in range(i + 1, j):
                masked[k] = "_"
            literals[i] = (j + 1, "".join(value))
            i = j + 1
            continue
        if ch == "`":
            stack.append(["tpl", 0])
        elif ch == "{":
            mode[1] += 1
        elif ch == "}":
            if len(stack) > 1 and mode[1] == 0:
                stack.pop()
            else:
                mode[1] -= 1
        i += 1
    if len(stack) > 1:
        rows.append(f"{where}: a template literal is not closed, so A13 cannot read the file")
        return None
    return "".join(code), "".join(masked), literals


def _matching_close(masked, open_idx):
    """The offset of the bracket that closes the one at `open_idx`, or None if it never closes."""
    depth = 0
    for k in range(open_idx, len(masked)):
        ch = masked[k]
        if ch in _JS_OPEN:
            depth += 1
        elif ch in _JS_CLOSE:
            depth -= 1
            if depth == 0:
                return k
    return None


def _top_level_entries(masked, start, end):
    """The (start, end) spans of the comma-separated entries in masked[start:end], trimmed.

    Only a comma at bracket depth 0 splits. An empty entry is returned as a span with start == end.
    """
    spans, depth, s = [], 0, start
    for k in range(start, end):
        ch = masked[k]
        if ch in _JS_OPEN:
            depth += 1
        elif ch in _JS_CLOSE:
            depth -= 1
        elif ch == "," and depth == 0:
            spans.append((s, k))
            s = k + 1
    spans.append((s, end))
    trimmed = []
    for a, b in spans:
        while a < b and masked[a].isspace():
            a += 1
        while b > a and masked[b - 1].isspace():
            b -= 1
        trimmed.append((a, b))
    return trimmed


def _js_bindings(masked, literals, config_dir):
    """`const` bindings an alias value may name (spec §4.4 step 3).

    `const <id> = '<lit>'` gives ("lit", lit). `const <id> = fileURLToPath(new URL('<lit>',
    import.meta.url))` gives ("path", <lit> resolved against the config's directory). Any other
    `const` is not a binding, so an alias that names it is unresolvable.
    """
    bindings = {}
    for m in _JS_CONST_STRING_RE.finditer(masked):
        lit = literals.get(m.end())
        if lit and _JS_STATEMENT_END_RE.match(masked, lit[0]):
            bindings[m.group(1)] = ("lit", lit[1])
    for m in _JS_CONST_URL_RE.finditer(masked):
        lit = literals.get(m.end())
        if lit is None or not _JS_URL_TAIL_RE.match(masked, lit[0]):
            continue
        if lit[1].startswith("/") or ":" in lit[1]:
            continue
        bindings[m.group(1)] = ("path", os.path.normpath(os.path.join(config_dir, lit[1])))
    return bindings


def _alias_value(masked, literals, bindings, vs, ve):
    """Classify the alias value in masked[vs:ve]: ("lit", s), ("path", p), or (None, None)."""
    if vs < ve and masked[vs] in "'\"" and literals.get(vs, (None,))[0] == ve:
        return ("lit", literals[vs][1])
    ident = masked[vs:ve]
    if _JS_IDENT_RE.fullmatch(ident) and ident in bindings:
        return bindings[ident]
    return (None, None)


def vitest_config_facts(text, config_rel, rows):
    """Return (aliases, tsconfig_off) for one vitest config file (spec §4.4).

    `aliases` is a sorted list of (key, target) pairs, `target` relative to the repository root and
    normalized (it can start with `..` when it leaves the root; the caller reports that). An alias
    whose value is a bare package name adds nothing. `tsconfig_off` is True when the code holds
    `tsconfig: false` as a whole property. Each form outside the grammar appends a row to `rows`.
    """
    scanned = _scan_js(text, config_rel, rows)
    if scanned is None:
        return [], False
    _code, masked, literals = scanned
    config_dir = os.path.dirname(config_rel)
    for m in _JS_IMPORT_RE.finditer(masked):
        lit = literals.get(m.end())
        if lit and lit[1].startswith(("./", "../")):
            rows.append(f"{config_rel}: imports {lit[1]!r}; A13 cannot read a config that is split across files")
    for m in _JS_SPLIT_CONFIG_RE.finditer(masked):
        rows.append(f"{config_rel}: uses `{m.group(1)}`; A13 cannot read a config that is split across files")
    for m in _JS_PROJECTS_RE.finditer(masked):
        start = m.end()
        end = _matching_close(masked, start) if masked[start:start + 1] == "[" else None
        if end is None:
            rows.append(f"{config_rel}: the `projects` value is not an array literal A13 can read")
            continue
        for s, e in _top_level_entries(masked, start + 1, end):
            if s < e and masked[s] in "'\"`":
                rows.append(
                    f"{config_rel}: `projects` holds a string entry; vitest loads it as another config or "
                    f"a glob, which A13 does not follow"
                )
    bindings = _js_bindings(masked, literals, config_dir)
    key_ends = [m.end() for m in _JS_ALIAS_WORD_RE.finditer(masked)]
    key_ends += [end for end, value in literals.values() if value == "alias" and _JS_KEY_COLON_RE.match(masked, end)]
    aliases = []
    for key_end in sorted(key_ends):
        colon = _JS_COLON_RE.match(masked, key_end)
        if colon is None:
            rows.append(f"{config_rel}: `alias` is used without a `:` value (a shorthand or a variable), which A13 cannot read")
            continue
        open_idx = colon.end()
        close_idx = _matching_close(masked, open_idx) if masked[open_idx:open_idx + 1] == "{" else None
        if close_idx is None:
            rows.append(f"{config_rel}: an `alias` value is not an object literal, so A13 cannot read it")
            continue
        entries = _top_level_entries(masked, open_idx + 1, close_idx)
        for idx, (s, e) in enumerate(entries):
            if s == e:
                if idx != len(entries) - 1:
                    rows.append(f"{config_rel}: an `alias` object holds an empty entry")
                continue
            if masked.startswith("...", s):
                rows.append(f"{config_rel}: an `alias` object holds a spread entry, which A13 cannot read")
                continue
            if masked[s] == "[":
                rows.append(f"{config_rel}: an `alias` object holds a computed key, which A13 cannot read")
                continue
            if masked[s] in "'\"" and s in literals:
                key_end_idx, key = literals[s]
            else:
                km = _JS_IDENT_RE.match(masked, s)
                if km is None:
                    rows.append(f"{config_rel}: an `alias` entry has a key A13 cannot read")
                    continue
                key, key_end_idx = km.group(0), km.end()
            sep = _JS_COLON_RE.match(masked, key_end_idx)
            if sep is None or sep.end() > e:
                rows.append(f"{config_rel}: the alias {key} has no `:` value, which A13 cannot read")
                continue
            kind, value = _alias_value(masked, literals, bindings, sep.end(), e)
            if kind == "lit" and value.startswith(("./", "../")):
                kind, value = "path", os.path.normpath(os.path.join(config_dir, value))
            if kind == "path":
                aliases.append((key, value))
            elif kind == "lit" and not key.startswith("@paigasus/"):
                continue
            else:
                rows.append(f"{config_rel}: the alias {key} has a value A13 cannot resolve")
    return sorted(set(aliases)), bool(_JS_TSCONFIG_OFF_RE.search(masked))
```

- [ ] **Step 4: Run the self-test and see it pass.**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "rc=$?"
```

Expected: `  OK   [parity] all twelve assertions fire on synthetic violations` and `rc=0`.

- [ ] **Step 5: Prove that two scanner states bite.** The file is not committed yet, so undo each mutation by hand. Do NOT use `git checkout`: it would also remove the new code.
  - In `_scan_js`, delete the seven-line `if ch == "/" and nxt == "/":` branch (from that line to its `continue`). Run the self-test. Expected (measured on a scratch copy), among the FAIL rows:

```text
  FAIL A13 parser, a `//` comment: unexpected rows ['ts/packages/k/vitest.config.ts: the alias @paigasus/x has a value A13 cannot resolve']
  FAIL A13 parser, quotes inside comments: unexpected rows ["ts/packages/k/vitest.config.ts: a ' string is not closed on its line, so A13 cannot read the file"]
  FAIL A13 parser, quotes inside comments: aliases [], expected [('a', 'ts/packages/k/a')]
  FAIL A13 parser, `tsconfig: false` in a comment: tsconfig_off is True, expected False
```

    Restore the branch.
  - In `_scan_js`, in the template state, change `            if ch == "\\":` to `            if False:`. Run the self-test. Expected:

```text
  FAIL A13 parser, a template literal: unexpected rows ['ts/packages/k/vitest.config.ts: a template literal is not closed, so A13 cannot read the file']
  FAIL A13 parser, a template literal: aliases [], expected [('a', 'ts/packages/k/a')]
```

    Restore the line.

  Then run the self-test again (`rc=0`) and `git diff --stat` (only `ci/affected-graph/cargo_moon_parity.py` changed).

- [ ] **Step 6: Run Ruff.** Expected: `All checks passed!`

- [ ] **Step 7: Commit.**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -F- <<'EOF'
test(ci): parse vitest configs for A13 with a single-pass scanner (SMA-736)

vitest_config_facts reads the aliases and the tsconfig: false flag of
one vitest config. The scanner keeps one state at a time, so a quote in
a comment or a // in a string cannot lose an alias. Any alias form
outside the grammar is a row, never a skip.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

### Task 3: Task selection and config lookup (spec §4.1, §4.2, T6)

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` — insert the selection block before `def moon_projects():` (after the Task 2 block); self-test rows before `    for f in failures:`.

**Interfaces:**
- Consumes: `MoonOutputError` (line 52).
- Produces: `VITEST_TOKEN_RE: re.Pattern`; `VITEST_CONFIG_NAMES: tuple[str, ...]`; `derive_vitest_tasks(projects: dict) -> set[str]`; `vitest_invocation_configs(target: str, blob: str, root: Path, own: str, rows: list[str]) -> list[str]`.

- [ ] **Step 1: Write the failing self-test rows.** Insert this block before `    for f in failures:` (after the Task 2 rows):

```python
    # A13 (SMA-736) — the vitest token, exercised directly (A10's `_var_sensitive` lesson).
    for blob, want in (
        ("pnpm exec vitest run", True),
        ("vitest run --passWithNoTests", True),
        ("pnpm exec vitest", True),
        ("set set -euo pipefail\npnpm exec vitest run --config vitest.containers.config.ts\n", True),
        ("touch a && ( cd b && wasm-pack build ) && pnpm exec vitest run", True),
        ("node vitest.config.ts", False),
        ("pnpm exec vitest-environment-x", False),
        ("pnpm add -D @vitest/coverage-v8", False),
        ("pnpm exec tsc -p tsconfig.json --noEmit", False),
    ):
        if bool(VITEST_TOKEN_RE.search(blob)) is not want:
            failures.append(
                f"VITEST_TOKEN_RE on {blob!r} is {not want} — the vitest token is wrong in the "
                f"{'false-negative' if want else 'false-positive'} direction"
            )

    # A13 — config lookup from the invocation (spec T6). Each row is (invocation, source dir,
    # expected configs, the one expected row or None for "no rows at all").
    a13_k = "ts/packages/k"
    a13_rel_row = (
        "t:test runs vitest with the config path $CFG, which A13 cannot resolve (it holds `$`, a glob "
        "character or a quote, or it leaves the repository)"
    )
    with tempfile.TemporaryDirectory() as tmp:
        lookup_root = Path(tmp)
        for rel in (
            "ts/packages/k/vitest.config.ts",
            "ts/packages/k/vitest.e2e.config.ts",
            "ts/packages/m/vitest.config.mts",
            "ts/packages/v/vite.config.ts",
            "ts/packages/v/vitest.config.js",
        ):
            (lookup_root / rel).parent.mkdir(parents=True, exist_ok=True)
            (lookup_root / rel).write_text("export default {};\n")
        (lookup_root / "ts/packages/n").mkdir(parents=True)
        e2e = ["ts/packages/k/vitest.e2e.config.ts"]
        for blob, own, want_configs, want_row in (
            ("pnpm exec vitest run --config vitest.e2e.config.ts", a13_k, e2e, None),
            ("pnpm exec vitest run --config=vitest.e2e.config.ts", a13_k, e2e, None),
            ("pnpm exec vitest run -c vitest.e2e.config.ts", a13_k, e2e, None),
            ("pnpm exec vitest run --config ./vitest.e2e.config.ts", a13_k, e2e, None),
            ("set set -euo pipefail\npnpm exec vitest run\npnpm exec vitest run --config vitest.e2e.config.ts\n", a13_k,
             ["ts/packages/k/vitest.config.ts", *e2e], None),
            ("pnpm exec vitest run --config vitest.ghost.config.ts", a13_k, [],
             "t:test runs vitest with ts/packages/k/vitest.ghost.config.ts, which does not exist"),
            ("pnpm exec vitest run --passWithNoTests", "ts/packages/n", [], None),
            ("pnpm exec vitest run", "ts/packages/m", ["ts/packages/m/vitest.config.mts"], None),
            ("pnpm exec vitest run", "ts/packages/v", ["ts/packages/v/vitest.config.js"], None),
            ("pnpm exec vitest run --root sub", a13_k, [],
             "t:test runs vitest with --root, so A13 cannot tell which config vitest reads"),
            ("pnpm exec vitest run -r sub", a13_k, [],
             "t:test runs vitest with -r, so A13 cannot tell which config vitest reads"),
            ("cd sub && pnpm exec vitest run", a13_k, [],
             "t:test changes directory with `cd` before it runs vitest, so A13 cannot tell which config vitest reads"),
            ("touch a && ( cd ../x && wasm-pack build . --out-dir o ) && pnpm exec vitest run", a13_k,
             ["ts/packages/k/vitest.config.ts"], None),
            ("pnpm exec vitest run --config $CFG", a13_k, [], a13_rel_row),
        ):
            rows = []
            got = vitest_invocation_configs("t:test", blob, lookup_root, own, rows)
            if got != want_configs or (want_row is None and rows) or (want_row is not None and want_row not in rows):
                failures.append(
                    f"vitest_invocation_configs({blob!r}, {own!r}) gave {got} and rows {rows}, expected "
                    f"{want_configs} and {'no row' if want_row is None else repr(want_row)}"
                )
```

- [ ] **Step 2: Run the self-test and see it fail.**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "rc=$?"
```

Expected: a traceback that ends with `NameError: name 'VITEST_TOKEN_RE' is not defined`, and `rc=1`.

- [ ] **Step 3: Add the selection code.** Insert this block immediately before the line `def moon_projects():` (so it follows the Task 2 block):

```python
# `vitest` as a bounded token, optionally behind `pnpm exec`. The boundaries are TSC_TOKEN_RE's, so
# `vitest.config.ts`, `vitest-environment-x` and `@vitest/coverage-v8` never match. A vitest call
# behind a wrapper script is invisible (spec N3); the task floor catches the loss of a known task.
VITEST_TOKEN_RE = re.compile(r"(^|[\s;&|(])(pnpm\s+exec\s+)?vitest(\s|$)")
# vitest 5.0.3's own lookup order when no `--config` is given (spec §2 item 4).
VITEST_CONFIG_NAMES = tuple(f"{stem}.config.{ext}" for stem in ("vitest", "vite") for ext in ("ts", "mts", "cts", "js", "mjs", "cjs"))
_VITEST_SPAN_END_RE = re.compile(r"&&|\|\||;|\||\n")
# `cd` or `pushd` as a command word. `--cwd` is an argument of another tool and does not match.
_VITEST_CD_RE = re.compile(r"(?:^|[\s;&|])(?:cd|pushd)(?=\s|$)")
# A balanced `( ... )` group: a subshell or a `$( ... )`. A `cd` inside one ends with the group,
# so it does not move the vitest call that follows. The real paigasus-kernel-ts:test has this shape.
_SUBSHELL_RE = re.compile(r"\([^()]*\)")
_VITEST_CONFIG_BAD_CHARS = frozenset("$*?[]{}'\"`")


def derive_vitest_tasks(projects):
    """Every `<pid>:<task>` of a `language: typescript` project whose resolved invocation runs vitest.

    Raises MoonOutputError if a task exposes none of a command, a script, or any args, exactly as
    derive_tsc_tasks does.
    """
    matched = set()
    for pid in sorted(projects):
        proj = projects[pid]
        if proj.get("language") != "typescript":
            continue
        invocations = proj.get("invocations") or {}
        for name in sorted(invocations):
            blob = invocations[name]
            if blob is None:
                raise MoonOutputError(
                    f"{pid}:{name} reported none of a `command`, a `script`, or any `args` — "
                    f"moon's output shape changed, so the vitest derivation cannot be evaluated"
                )
            if VITEST_TOKEN_RE.search(blob):
                matched.add(f"{pid}:{name}")
    return matched


def vitest_invocation_configs(target, blob, root, own, rows):
    """The config files that the vitest calls in `blob` read, as repository-relative paths (spec §4.2).

    For each vitest call, the words after the token up to the next `&&`, `||`, `;`, `|` or newline
    are read. `--config <f>`, `--config=<f>` and `-c <f>` name a config relative to `own`, the task's
    source directory. With no flag, vitest's own lookup applies in `own`; no file there means no
    config, which is not a row. `--root`, `-r`, a `cd` before the call outside a subshell, and a
    config path with `$`, a glob character or a quote are rows, never skips.
    """
    configs = []
    for m in VITEST_TOKEN_RE.finditer(blob):
        prefix = blob[:m.start()]
        while True:
            stripped = _SUBSHELL_RE.sub(" ", prefix)
            if stripped == prefix:
                break
            prefix = stripped
        if _VITEST_CD_RE.search(prefix):
            rows.append(f"{target} changes directory with `cd` before it runs vitest, so A13 cannot tell which config vitest reads")
            continue
        end = _VITEST_SPAN_END_RE.search(blob, m.end())
        words = blob[m.end():end.start() if end else len(blob)].split()
        explicit, refused = [], False
        i = 0
        while i < len(words):
            word = words[i]
            if word in ("--root", "-r") or word.startswith("--root="):
                rows.append(f"{target} runs vitest with {word.split('=', 1)[0]}, so A13 cannot tell which config vitest reads")
                refused = True
                break
            if word in ("--config", "-c"):
                if i + 1 == len(words):
                    rows.append(f"{target} runs vitest with {word} and no path")
                    refused = True
                    break
                explicit.append(words[i + 1])
                i += 2
                continue
            if word.startswith("--config="):
                explicit.append(word[len("--config="):])
            i += 1
        if refused:
            continue
        for raw in explicit:
            rel = os.path.normpath(os.path.join(own, raw))
            if any(c in _VITEST_CONFIG_BAD_CHARS for c in raw) or os.path.isabs(raw) or rel.startswith("../"):
                rows.append(
                    f"{target} runs vitest with the config path {raw}, which A13 cannot resolve (it holds `$`, "
                    f"a glob character or a quote, or it leaves the repository)"
                )
            elif not (root / rel).is_file():
                rows.append(f"{target} runs vitest with {rel}, which does not exist")
            else:
                configs.append(rel)
        if not explicit:
            for name in VITEST_CONFIG_NAMES:
                if (root / own / name).is_file():
                    configs.append(f"{own}/{name}")
                    break
    return list(dict.fromkeys(configs))
```

- [ ] **Step 4: Run the self-test and see it pass.**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "rc=$?"
```

Expected: `  OK   [parity] all twelve assertions fire on synthetic violations` and `rc=0`.

- [ ] **Step 5: Prove the subshell rule bites.** In `vitest_invocation_configs`, delete the five-line `while True:` loop that applies `_SUBSHELL_RE`. Run the self-test. Expected (measured on a scratch copy):

```text
  FAIL vitest_invocation_configs('touch a && ( cd ../x && wasm-pack build . --out-dir o ) && pnpm exec vitest run', 'ts/packages/k') gave [] and rows ['t:test changes directory with `cd` before it runs vitest, so A13 cannot tell which config vitest reads'], expected ['ts/packages/k/vitest.config.ts'] and no row
```

Restore the loop by hand (do NOT use `git checkout`; the new code is not committed yet), and run the self-test again: `rc=0`.

- [ ] **Step 6: Run Ruff.** Expected: `All checks passed!`

- [ ] **Step 7: Commit.**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -F- <<'EOF'
test(ci): derive the vitest tasks and their config files for A13 (SMA-736)

derive_vitest_tasks selects every typescript task that runs vitest.
vitest_invocation_configs finds the config files from --config, -c or
vitest's own lookup. A cd before the call outside a subshell, --root and
a config path with $, a glob character or a quote are rows.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

### Task 4: Git's tracked set as an infrastructure call (spec §4.5, R3)

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` — imports (lines 39-49), `INFRA_ERRORS` (lines 68-74), self-test rows before `    for f in failures:`.
- Modify: `ci/affected-graph/task_inputs.py` — imports (lines 11-15), `_git` (lines 253-273).

**Interfaces:**
- Consumes: `task_inputs.tracked_files(root: Path) -> set[str]` (task_inputs.py line 276); `task_inputs.MoonOutputError`.
- Produces: `cargo_moon_parity.INFRA_ERRORS` holds `task_inputs.MoonOutputError`; `task_inputs._git(args, root)` runs git without `GIT_DIR` and `GIT_INDEX_FILE`.

- [ ] **Step 1: Write the failing self-test rows.** Insert this block before `    for f in failures:` (after the Task 3 rows):

```python
    # A13 (SMA-736 §4.5) — the tracked-file plumbing that main() uses. tracked_files raises
    # task_inputs' OWN MoonOutputError, a different class from this file's. It must be in
    # INFRA_ERRORS, or a failed `git ls-files` exits 1 with a traceback instead of rc 2.
    with tempfile.TemporaryDirectory() as tmp:
        try:
            task_inputs.tracked_files(Path(tmp))
        except INFRA_ERRORS:
            pass
        except Exception as exc:
            failures.append(
                f"tracked_files outside a repository raised {type(exc).__name__}, which INFRA_ERRORS does "
                f"not catch, so main() would exit 1 instead of rc 2"
            )
        else:
            failures.append("tracked_files outside a repository returned a set instead of raising")
    # ...and the git call must ignore an inherited GIT_DIR and GIT_INDEX_FILE (a git hook sets them).
    a13_repo_root = Path(__file__).resolve().parents[2]
    a13_saved_env = {k: os.environ.get(k) for k in ("GIT_DIR", "GIT_INDEX_FILE")}
    os.environ["GIT_DIR"] = "/nonexistent/sma-736-git-dir"
    os.environ["GIT_INDEX_FILE"] = "/nonexistent/sma-736-index"
    try:
        a13_tracked_real = task_inputs.tracked_files(a13_repo_root)
    except Exception as exc:
        a13_tracked_real = None
        failures.append(f"tracked_files used an inherited GIT_DIR or GIT_INDEX_FILE: {exc}")
    finally:
        for k, v in a13_saved_env.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v
    if a13_tracked_real is not None and "ci/affected-graph/cargo_moon_parity.py" not in a13_tracked_real:
        failures.append("tracked_files does not list this file, so it did not read this repository")
```

- [ ] **Step 2: Run the self-test and see it fail.**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "rc=$?"
```

Expected: a traceback that ends with `NameError: name 'task_inputs' is not defined`, and `rc=1`.

- [ ] **Step 3: Import the sibling module.** In `cargo_moon_parity.py`, replace:

```python
import tomllib
from pathlib import Path
```

with:

```python
import tomllib
from pathlib import Path

# SMA-736: A13 reads git's tracked set through repo:input-liveness's own helper. Python puts this
# script's directory (ci/affected-graph/) first on sys.path, so the sibling module imports directly.
import task_inputs
```

- [ ] **Step 4: Run the self-test and see the two plumbing rows fail.**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "rc=$?"
```

Expected (measured on a scratch copy while this plan was written), and `rc=1`:

```text
  FAIL tracked_files outside a repository raised MoonOutputError, which INFRA_ERRORS does not catch, so main() would exit 1 instead of rc 2
  FAIL tracked_files used an inherited GIT_DIR or GIT_INDEX_FILE: `git ls-files` failed with rc 128: fatal: not a git repository: '/nonexistent/sma-736-git-dir'
negative-control FAILED: the parity gate can pass vacuously
```

- [ ] **Step 5: Add the class to `INFRA_ERRORS`.** In `cargo_moon_parity.py`, replace:

```python
    OSError,
    MoonOutputError,
)
```

with:

```python
    OSError,
    MoonOutputError,
    # SMA-736: task_inputs.tracked_files raises task_inputs' own MoonOutputError class.
    task_inputs.MoonOutputError,
)
```

- [ ] **Step 6: Clear the two git variables in `task_inputs._git`.** In `ci/affected-graph/task_inputs.py`, replace:

```python
import json
import re
```

with:

```python
import json
import os
import re
```

Then replace the end of the `_git` docstring and the `subprocess.run` call:

```python
    reads as `dead` — a false red, the safe direction. classify() is the real defense there.
    """
    proc = subprocess.run(
        ["git", "-c", "core.quotePath=false", *args],
        cwd=root, capture_output=True, text=True,
    )
```

with:

```python
    reads as `dead` — a false red, the safe direction. classify() is the real defense there.

    `GIT_DIR` and `GIT_INDEX_FILE` are removed from the environment (SMA-736). A git hook sets them,
    and an inherited value would make `ls-files` read another repository or index than `root`'s.
    """
    env = {k: v for k, v in os.environ.items() if k not in ("GIT_DIR", "GIT_INDEX_FILE")}
    proc = subprocess.run(
        ["git", "-c", "core.quotePath=false", *args],
        cwd=root, capture_output=True, text=True, env=env,
    )
```

This also changes `repo:input-liveness`, which uses the same `_git`. The change is safe there: the gate always runs git in `root`.

- [ ] **Step 7: Run both self-tests and see them pass.**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "parity rc=$?"
python3 ci/affected-graph/task_inputs.py --self-test; echo "input-liveness rc=$?"
```

Expected: `  OK   [parity] all twelve assertions fire on synthetic violations`, `parity rc=0`, and `input-liveness rc=0`.

- [ ] **Step 8: Run Ruff.** Expected: `All checks passed!`

- [ ] **Step 9: Commit.**

```bash
git add ci/affected-graph/cargo_moon_parity.py ci/affected-graph/task_inputs.py
git commit -F- <<'EOF'
feat(ci): read git's tracked set in the parity gate as an infrastructure call (SMA-736)

A13 needs the set of tracked files. The gate reads it through
task_inputs.tracked_files. That function raises its own MoonOutputError
class, so INFRA_ERRORS now holds it, and a git failure is rc 2, not a
traceback. The git call ignores an inherited GIT_DIR and GIT_INDEX_FILE.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

### Task 5: The required set, `check_ts_vitest_inputs`, and its registration (spec §4.3, §4.6-§4.9, T1-T4, T7-T10, T12)

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` — the A13 check block before `def moon_projects():` (after the Task 3 block); self-test rows before `    for f in failures:`; the header comment (lines 34-37); `EXPECTED_FINDING_KEYS` (line 5072); `collect_findings` (lines 5075-5233); `main` (lines 5236-5269); the arity call (line 2747); the OK message (line 5055).
- Modify: this plan — the Measurements section (M1, M2, M3).

**Interfaces:**
- Consumes: `workspace_package_inputs` (Task 1); `_scan_js`, `vitest_config_facts` (Task 2); `derive_vitest_tasks`, `vitest_invocation_configs` (Task 3); `task_inputs.tracked_files` (Task 4); `derive_ffi_tasks(projects)` (line 697); `ts_tsc_analysis(projects, root)` (line 2329); `ts_workspace_packages(root, rows)` (line 2195); `ts_closure(root, own_dir, names, rows)` (line 2215); `_read_package_json(root, rel_dir, rows)` (line 2171); `PROTO_PACKAGE`, `PROTO_GENERATE_DEP`, `TS_BASE_TSCONFIG` (lines 2122-2127); the self-test helpers `_a12_ts` and `_a12_copy` (defined inside `self_test()` with the A12 rows).
- Produces: `TS_LOCKFILE`, `REQUIRED_VITEST_TASKS`, `REQUIRED_VITEST_ALIASES`, `AFFECTED_SMOKE_PROJECT`, `AFFECTED_SMOKE_TASK`; `vitest_required_inputs(root, own, closure, facts, rows) -> set[str]`; `check_ts_vitest_inputs(projects, root, tracked, floor=REQUIRED_VITEST_TASKS, alias_floor=REQUIRED_VITEST_ALIASES) -> list[str]`; `collect_findings(projects, crates, root, tracked)`; the findings key `a13`.

- [ ] **Step 1: Write the failing self-test rows.** Insert this block before `    for f in failures:` (after the Task 4 rows). It uses `_a12_ts` and `_a12_copy`, which the A12 rows define earlier in `self_test()`:

```python
    # A13 (SMA-736) — the whole check. Two halves, like A12: a file tree written to a tmp root, and
    # moon's resolved view of the vitest tasks. `a13_tracked` stands in for `git ls-files`; the one
    # path it leaves out, rs/crates/bindings/wb/.out/wb.js, is the kernel's scratch alias target (D3).
    if not REQUIRED_VITEST_TASKS:
        failures.append("REQUIRED_VITEST_TASKS is empty — A13's task floor would assert nothing")
    if not REQUIRED_VITEST_ALIASES:
        failures.append("REQUIRED_VITEST_ALIASES is empty — A13's alias floor would assert nothing")

    a13_files = {
        "ts/tsconfig.base.json": '{"compilerOptions": {"strict": true}}\n',
        "ts/packages/kernel/package.json": json.dumps({
            "name": "@paigasus/kernel",
            "dependencies": {
                "@paigasus/node-bindings": "file:../../../rs/crates/bindings/nb",
                "@paigasus/wasm": "file:../../../rs/crates/bindings/wb",
            },
            "exports": {".": "./src/wasm.ts"},
        }),
        "ts/packages/kernel/src/wasm.ts": "export {};\n",
        # JSONC, as the real kernel and ui tsconfig files are. A plain json.loads fails on it.
        "ts/packages/kernel/tsconfig.json": '// JSONC, as in the real kernel package.\n{"extends": "../../tsconfig.base.json"}\n',
        "ts/packages/kernel/vitest.config.ts": (
            "import { fileURLToPath } from 'node:url';\n"
            "import { defineConfig } from 'vitest/config';\n"
            "const nb = fileURLToPath(new URL('../../../rs/crates/bindings/nb/index.js', import.meta.url));\n"
            "const wb = fileURLToPath(new URL('../../../rs/crates/bindings/wb/.out/wb.js', import.meta.url));\n"
            "export default defineConfig({\n"
            "  test: { projects: [\n"
            "    { resolve: { alias: { '@paigasus/node-bindings': nb } } },\n"
            "    { resolve: { alias: { '@paigasus/wasm': wb } } },\n"
            "  ] },\n"
            "});\n"
        ),
        "rs/crates/bindings/nb/package.json": json.dumps({"name": "@paigasus/node-bindings", "files": ["index.js", "index.d.ts"]}),
        "rs/crates/bindings/nb/index.js": "",
        "rs/crates/bindings/nb/index.d.ts": "",
        # `./wb.js` proves the normalization: the clean fixture declares `rs/crates/bindings/wb/wb.js`.
        "rs/crates/bindings/wb/package.json": json.dumps({"name": "@paigasus/wasm", "files": ["./wb.js", "wb_bg.wasm", "wb.d.ts"]}),
        "rs/crates/bindings/wb/wb.js": "",
        "rs/crates/bindings/wb/wb_bg.wasm": "",
        "rs/crates/bindings/wb/wb.d.ts": "",
        # No tsconfig.json: a missing tsconfig adds nothing.
        "ts/packages/proto/package.json": json.dumps({"name": "@paigasus/proto", "exports": {".": "./src/index.ts"}}),
        "ts/packages/core/package.json": json.dumps({
            "name": "@paigasus/console-core",
            "dependencies": {"@paigasus/kernel": "workspace:*"},
            "devDependencies": {"@paigasus/proto": "workspace:*"},
            "exports": {".": "./src/index.ts", "./testing": "./testing/index.ts"},
        }),
        "ts/packages/core/tsconfig.json": '{"extends": "../../tsconfig.base.json"}\n',
        # One alias inside the own package (adds nothing) and one tracked target outside it (demanded
        # as a literal, although ts/packages/kernel/src/**/* covers it).
        "ts/packages/core/vitest.config.ts": (
            "import { defineConfig } from 'vitest/config';\n"
            "export default defineConfig({\n"
            "  resolve: { alias: { 'server-only': './tests/stub.ts', '@paigasus/kernel': '../kernel/src/wasm.ts' } },\n"
            "});\n"
        ),
        "ts/packages/core/vitest.e2e.config.ts": (
            "import { defineConfig } from 'vitest/config';\n"
            "export default defineConfig({ test: { include: ['tests/e2e/**'] } });\n"
        ),
        "ts/apps/app/package.json": json.dumps({"name": "@paigasus/app", "dependencies": {"@paigasus/console-core": "workspace:*"}}),
        # A package-specifier `extends` is a row, but only when A13 reads the file. The app config
        # sets `tsconfig: false`, so the clean fixture never reads it.
        "ts/apps/app/tsconfig.json": '{"extends": "@paigasus/next-config/tsconfig-app"}\n',
        "ts/apps/app/vitest.config.ts": (
            "import { defineConfig } from 'vitest/config';\n"
            "const oxc = { tsconfig: false } as const;\n"
            "export default defineConfig({ oxc });\n"
        ),
    }
    a13_tracked = frozenset(a13_files)
    a13_lock = TS_LOCKFILE
    a13_nb = ["rs/crates/bindings/nb/package.json", "rs/crates/bindings/nb/index.js"]
    a13_wb = ["rs/crates/bindings/wb/package.json", "rs/crates/bindings/wb/wb.js", "rs/crates/bindings/wb/wb_bg.wasm"]
    a13_core_files = [
        a13_lock, "ts/packages/kernel/package.json", "ts/packages/proto/package.json", *a13_nb, *a13_wb,
        "ts/packages/core/tsconfig.json", "ts/packages/kernel/tsconfig.json", TS_BASE_TSCONFIG,
    ]
    a13_core_globs = ["ts/packages/kernel/src/**/*", "ts/packages/proto/src/**/*"]
    # The real kernel test's shape: an FFI build, a `cd` inside a subshell, then vitest.
    a13_ffi = (
        "touch touch x && pnpm exec napi build --platform && "
        "( cd ../../../rs/crates/bindings/wb && wasm-pack build . --out-dir .out ) && pnpm exec vitest run"
    )
    a13 = {
        "k-ts": _a12_ts("ts/packages/kernel", {
            "test": (a13_ffi, [], [
                a13_lock, "ts/packages/kernel/vitest.config.ts", *a13_nb, *a13_wb,
                "ts/packages/kernel/tsconfig.json", TS_BASE_TSCONFIG,
            ], []),
        }),
        "c-ts": _a12_ts("ts/packages/core", {
            "test": (
                "pnpm exec vitest run --passWithNoTests", [PROTO_GENERATE_DEP],
                [*a13_core_files, "ts/packages/core/vitest.config.ts", "ts/packages/kernel/src/wasm.ts"], a13_core_globs,
            ),
            "test-e2e": (
                "set set -euo pipefail\npnpm exec vitest run --config vitest.e2e.config.ts\n", [PROTO_GENERATE_DEP],
                [*a13_core_files, "ts/packages/core/vitest.e2e.config.ts"], a13_core_globs,
            ),
        }),
        # No config file: vitest runs with defaults (as paigasus-proto-ts:test does).
        "p-ts": _a12_ts("ts/packages/proto", {
            "test": ("pnpm exec vitest run --passWithNoTests", [PROTO_GENERATE_DEP], [a13_lock], []),
        }),
        "app-ts": _a12_ts("ts/apps/app", {
            "test": (
                "pnpm exec vitest run", [PROTO_GENERATE_DEP],
                [a13_lock, "ts/apps/app/vitest.config.ts", "ts/packages/core/package.json", "ts/packages/kernel/package.json",
                 "ts/packages/proto/package.json", *a13_nb, *a13_wb],
                [*a13_core_globs, "ts/packages/core/src/**/*", "ts/packages/core/testing/**/*"],
            ),
        }),
        # A vitest call in a NON-typescript project, under-declared on purpose. A13 must not examine it.
        "x-py": _a12_ts("py/x", {"test": ("pnpm exec vitest run", [], [], [])}, language="python"),
        "repo": _a12_ts(".", {
            "affected-smoke": ("true", [], [], ["ts/packages/*/vitest*.config.*", "ts/apps/*/vitest*.config.*"]),
        }, language="bash"),
    }
    a13_floor = ("k-ts:test", "c-ts:test", "c-ts:test-e2e", "app-ts:test")
    a13_alias_floor = {"k-ts:test": {"@paigasus/node-bindings", "@paigasus/wasm"}}

    def _a13(fixture=None, files=None, tracked=None, **kw):
        # `files` maps a path to its text; None deletes the path from the tree.
        kw.setdefault("floor", a13_floor)
        kw.setdefault("alias_floor", a13_alias_floor)
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            for rel, text in (a13_files if files is None else files).items():
                if text is None:
                    continue
                (base / rel).parent.mkdir(parents=True, exist_ok=True)
                (base / rel).write_text(text)
            return check_ts_vitest_inputs(
                a13 if fixture is None else fixture, base,
                a13_tracked if tracked is None else tracked, **kw,
            )

    # T1. Clean. This also proves: the subshell `cd` in k-ts is not a row; the scratch alias with the
    # matching --out-dir passes (D3); the in-package alias adds nothing; no `.d.ts` is demanded;
    # `./wb.js` is normalized; the app's `tsconfig: false` skips its tsconfig; x-py is ignored.
    rows = _a13()
    if rows != []:
        failures.append(f"A13 reported violations on a complete fixture: {rows}")

    # T2. One mutation per part of `want`, each read PER TASK: c-ts:test-e2e keeps every entry that
    # c-ts:test loses, so a union across the project's tasks would hide the shortfall (A7-g's lesson).
    for pid, task, bucket, entry in (
        ("c-ts", "test", "task_inputs", a13_lock),
        ("c-ts", "test", "task_inputs", "ts/packages/core/vitest.config.ts"),
        ("app-ts", "test", "task_input_globs", "ts/packages/kernel/src/**/*"),
        ("c-ts", "test", "task_inputs", "ts/packages/proto/package.json"),
        ("app-ts", "test", "task_input_globs", "ts/packages/core/testing/**/*"),
        ("c-ts", "test", "task_inputs", "rs/crates/bindings/wb/wb_bg.wasm"),
        ("c-ts", "test", "task_inputs", "rs/crates/bindings/wb/wb.js"),
        ("c-ts", "test", "task_inputs", "ts/packages/kernel/tsconfig.json"),
        ("k-ts", "test", "task_inputs", "ts/packages/kernel/tsconfig.json"),
        ("k-ts", "test", "task_inputs", TS_BASE_TSCONFIG),
        ("c-ts", "test", "task_inputs", "ts/packages/kernel/src/wasm.ts"),
    ):
        broken = _a12_copy(a13)
        broken[pid][bucket][task].remove(entry)
        rows = _a13(broken)
        if f"{pid}:{task} inputs omit {entry}" not in rows:
            failures.append(f"A13 did not demand {entry} of {pid}:{task}")
        if pid == "c-ts" and any(r.startswith("c-ts:test-e2e ") for r in rows):
            failures.append(f"A13 blamed c-ts:test-e2e for {entry}, a shortfall that lives on c-ts:test")

    # T3. An untracked alias target: on a task that is not an FFI task, and on an FFI task that builds
    # somewhere else. The FFI task WITH the matching --out-dir is the clean row above.
    files = dict(a13_files)
    files["ts/packages/core/vitest.config.ts"] = "export default { resolve: { alias: { '@paigasus/kernel': '../kernel/.out/k.js' } } };\n"
    if (
        "c-ts:test aliases @paigasus/kernel to the untracked path ts/packages/kernel/.out/k.js, but the task does not "
        "build it there (no FFI build with --out-dir ts/packages/kernel/.out)"
    ) not in _a13(files=files):
        failures.append("A13 accepted an untracked alias target on a task that is not an FFI task")
    broken = _a12_copy(a13)
    broken["k-ts"]["invocations"]["test"] = a13_ffi.replace("--out-dir .out", "--out-dir other")
    if (
        "k-ts:test aliases @paigasus/wasm to the untracked path rs/crates/bindings/wb/.out/wb.js, but the task does "
        "not build it there (no FFI build with --out-dir rs/crates/bindings/wb/.out)"
    ) not in _a13(broken):
        failures.append("A13 accepted an untracked alias target that the FFI task builds somewhere else")

    # T4. An alias to a @paigasus/ package outside the closure. (An alias inside the own package is
    # the clean row's `server-only`, which demands nothing.)
    files = dict(a13_files)
    files["ts/packages/core/vitest.config.ts"] = "export default { resolve: { alias: { '@paigasus/ghost': '../kernel/src/wasm.ts' } } };\n"
    if (
        "c-ts:test aliases @paigasus/ghost in ts/packages/core/vitest.config.ts, but @paigasus/ghost is not in its "
        "package.json closure"
    ) not in _a13(files=files):
        failures.append("A13 accepted an alias to a @paigasus/ package outside the closure")

    # T7. The proto dep, for a closure reader and for proto itself.
    broken = _a12_copy(a13)
    broken["app-ts"]["tasks"]["test"] = []
    broken["p-ts"]["tasks"]["test"] = []
    rows = _a13(broken)
    for target in ("app-ts:test", "p-ts:test"):
        if not any(r.startswith(f"{target} deps omit {PROTO_GENERATE_DEP}") for r in rows):
            failures.append(f"A13 did not demand {PROTO_GENERATE_DEP} of {target}")
    # T7. The task floor: no task matches vitest, so every per-task row goes quiet by itself.
    broken = _a12_copy(a13)
    for proj in broken.values():
        proj["invocations"] = dict.fromkeys(proj["invocations"], "pnpm exec jest")
    if not any(r.startswith("FLOOR: k-ts:test is not matched by the vitest token") for r in _a13(broken)):
        failures.append("A13's task floor did not fire when no task matches vitest")
    # T7. The alias floor: the kernel config loses one binding alias.
    files = dict(a13_files)
    files["ts/packages/kernel/vitest.config.ts"] = files["ts/packages/kernel/vitest.config.ts"].replace(
        "{ resolve: { alias: { '@paigasus/node-bindings': nb } } },", "{},"
    )
    if not any(r.startswith("FLOOR: k-ts:test no longer aliases @paigasus/node-bindings") for r in _a13(files=files)):
        failures.append("A13's alias floor did not fire when the kernel config lost an alias")
    # T7. A task with no input bucket is a violation, never a skip.
    broken = _a12_copy(a13)
    del broken["c-ts"]["task_input_globs"]["test"]
    if not any(r.startswith("c-ts:test reported no `inputFiles`/`inputGlobs`") for r in _a13(broken)):
        failures.append("A13 skipped a task that reported no input bucket")
    # T7. A None invocation is moon telling us nothing. That is infra, exactly as in A5 and A12.
    broken = _a12_copy(a13)
    broken["c-ts"]["invocations"]["test"] = None
    try:
        derive_vitest_tasks(broken)
    except MoonOutputError:
        pass
    else:
        failures.append("derive_vitest_tasks accepted a task with no command, script or args")

    # T8. Binding `files` shapes. Each is a row, never a skip.
    for wb_manifest, want_row in (
        ({"name": "@paigasus/wasm"},
         "rs/crates/bindings/wb/package.json has no `files` key, so A13 cannot tell which files vitest loads from the binding"),
        ({"name": "@paigasus/wasm", "files": "wb.js"}, "rs/crates/bindings/wb/package.json `files` is not a list"),
        ({"name": "@paigasus/wasm", "files": [1]},
         "rs/crates/bindings/wb/package.json `files` holds an entry of type int, not a string"),
        ({"name": "@paigasus/wasm", "files": ["*.js"]},
         "rs/crates/bindings/wb/package.json `files` entry '*.js' holds a glob character; A13 needs literal entries"),
    ):
        files = dict(a13_files)
        files["rs/crates/bindings/wb/package.json"] = json.dumps(wb_manifest)
        if want_row not in _a13(files=files):
            failures.append(f"A13 did not report {want_row!r}")
    files = dict(a13_files)
    files["rs/crates/bindings/wb/wb_bg.wasm"] = None
    files["rs/crates/bindings/wb/wb_bg.wasm/inner"] = ""
    if "rs/crates/bindings/wb/package.json `files` entry 'wb_bg.wasm' is a directory; A13 needs file entries" not in _a13(files=files):
        failures.append("A13 accepted a `files` entry that is a directory")

    # T9. tsconfig `extends` forms A13 does not follow are rows.
    for extends, shown in (
        ('"@paigasus/next-config/tsconfig-app"', "'@paigasus/next-config/tsconfig-app'"),
        ('["../../tsconfig.base.json"]', "['../../tsconfig.base.json']"),
    ):
        files = dict(a13_files)
        files["ts/packages/core/tsconfig.json"] = '{"extends": ' + extends + "}\n"
        want_row = (
            f"ts/packages/core/tsconfig.json: `extends` is {shown}; A13 follows only a relative path, so add a "
            f"rule for this form on purpose (SMA-736)"
        )
        if want_row not in _a13(files=files):
            failures.append(f"A13 did not report the `extends` form {extends}")
    # T9. `tsconfig: false` is what skips the app tsconfig: without it, A13 reads the file.
    files = dict(a13_files)
    files["ts/apps/app/vitest.config.ts"] = "export default {};\n"
    if not any(r.startswith("ts/apps/app/tsconfig.json: `extends` is '@paigasus/next-config/tsconfig-app'") for r in _a13(files=files)):
        failures.append("A13 did not read the app tsconfig once its config stopped setting `tsconfig: false`")
    # T9. EVERY config must set it: one of two is not enough.
    files = dict(a13_files)
    files["ts/apps/app/vitest.other.config.ts"] = "export default {};\n"
    broken = _a12_copy(a13)
    broken["app-ts"]["invocations"]["test"] = "pnpm exec vitest run && pnpm exec vitest run --config vitest.other.config.ts"
    broken["app-ts"]["task_inputs"]["test"].append("ts/apps/app/vitest.other.config.ts")
    if not any(r.startswith("ts/apps/app/tsconfig.json: `extends`") for r in _a13(broken, files=files)):
        failures.append("A13 skipped the tsconfig of a task where only one of two configs sets `tsconfig: false`")
    # T9. A task with NO config file still reads its tsconfig ("every config" is not vacuously true).
    files = dict(a13_files)
    files["ts/packages/proto/tsconfig.json"] = '{"extends": "../../tsconfig.base.json"}\n'
    if "p-ts:test inputs omit ts/packages/proto/tsconfig.json" not in _a13(files=files):
        failures.append("A13 skipped the tsconfig of a task that has no vitest config file")

    # T10. The gate reachability check.
    broken = _a12_copy(a13)
    broken["repo"]["task_input_globs"]["affected-smoke"] = ["ts/packages/*/vitest.config.ts"]
    rows = _a13(broken)
    for cfg in ("ts/packages/core/vitest.e2e.config.ts", "ts/apps/app/vitest.config.ts"):
        if f"{cfg} matches no input of repo:affected-smoke, so an edit to it alone does not schedule the gate that reads it" not in rows:
            failures.append(f"A13 did not report that {cfg} cannot schedule repo:affected-smoke")
    broken = _a12_copy(a13)
    del broken["repo"]["task_input_globs"]["affected-smoke"]
    if not any(r.startswith("repo:affected-smoke reported no `inputGlobs`") for r in _a13(broken)):
        failures.append("A13 skipped the reachability check when repo:affected-smoke reported no globs")

    # T12. A broken manifest prints once. With no `tsc` task, A13 reports a broken binding manifest.
    # With a `tsc` task whose closure reaches it, A12a reports it and A13 drops it.
    files = dict(a13_files)
    files["rs/crates/bindings/wb/package.json"] = "{"
    if not any(r.startswith("rs/crates/bindings/wb/package.json is not valid JSON") for r in _a13(files=files)):
        failures.append("A13 did not report a broken manifest that no `tsc` task reads")
    broken = _a12_copy(a13)
    broken["c-ts"]["invocations"]["typecheck"] = "pnpm exec tsc -p tsconfig.json --noEmit"
    broken["c-ts"]["tasks"]["typecheck"] = [PROTO_GENERATE_DEP]
    broken["c-ts"]["task_inputs"]["typecheck"] = []
    broken["c-ts"]["task_input_globs"]["typecheck"] = []
    if any(r.startswith("rs/crates/bindings/wb/package.json") for r in _a13(broken, files=files)):
        failures.append("A13 printed a broken manifest again that A12a already reports")
```

- [ ] **Step 2: Run the self-test and see it fail.**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "rc=$?"
```

Expected: a traceback that ends with `NameError: name 'REQUIRED_VITEST_TASKS' is not defined`, and `rc=1`.

- [ ] **Step 3: Add the check.** Insert this block immediately before the line `def moon_projects():` (so it follows the Task 3 block):

```python
TS_LOCKFILE = "ts/pnpm-lock.yaml"
# The gate that reads the vitest configs. A13 asserts that each config it reads matches one of this
# task's inputs, so a config with a new name or in a new place cannot hide from the gate (spec §4.6).
AFFECTED_SMOKE_PROJECT = "repo"
AFFECTED_SMOKE_TASK = "affected-smoke"
_OUT_DIR_RE = re.compile(r"--out-dir(?:=|\s+)([^\s;&|)]+)")

# A13's anti-vacuity floors (spec §4.7). A13 asserts CONTAINMENT, and a containment check whose
# `want` set empties passes having asserted nothing (A7's lesson). The task floor catches a
# derivation that stops matching a known task. The alias floor catches a parser that stops finding
# the kernel's two binding aliases.
REQUIRED_VITEST_TASKS = (
    "paigasus-kernel-ts:test",
    "paigasus-console-core-ts:test",
    "iam-console-ts:test",
    "gateway-console-ts:test",
    "paigasus-auth-ts:test-e2e",
)
REQUIRED_VITEST_ALIASES = {"paigasus-kernel-ts:test": {"@paigasus/node-bindings", "@paigasus/wasm"}}


def _binding_runtime_inputs(root, pkg_dir, data, rows):
    """The files vitest loads from one `file:` binding: its package.json and each `files` entry that
    is not a `.d.ts` (spec §4.3 item 4). vitest runs the glue and the `.wasm`; it never reads a `.d.ts`.
    """
    want = {f"{pkg_dir}/package.json"}
    if "files" not in data:
        rows.append(f"{pkg_dir}/package.json has no `files` key, so A13 cannot tell which files vitest loads from the binding")
        return want
    files = data["files"]
    if not isinstance(files, list):
        rows.append(f"{pkg_dir}/package.json `files` is not a list")
        return want
    for entry in files:
        if not isinstance(entry, str):
            rows.append(f"{pkg_dir}/package.json `files` holds an entry of type {type(entry).__name__}, not a string")
            continue
        if any(c in entry for c in "*?[]{}!"):
            rows.append(f"{pkg_dir}/package.json `files` entry {entry!r} holds a glob character; A13 needs literal entries")
            continue
        rel = os.path.normpath(entry)
        if rel.endswith(".d.ts"):
            continue
        if (root / pkg_dir / rel).is_dir():
            rows.append(f"{pkg_dir}/package.json `files` entry {entry!r} is a directory; A13 needs file entries")
            continue
        want.add(f"{pkg_dir}/{rel}")
    return want


def _tsconfig_chain(root, pkg_dir, rows):
    """`<pkg_dir>/tsconfig.json` and each file its relative `extends` chain reaches (spec §4.3 item 5).

    A missing tsconfig.json adds nothing. The file is JSONC (the real kernel and ui tsconfig files
    hold `//` comments), so `_scan_js` removes the comments before `json.loads`. An `extends` that is
    not a relative path (a package specifier, an array) is a row, never a skip.
    """
    rel = f"{pkg_dir}/tsconfig.json"
    if not (root / rel).is_file():
        return []
    chain = []
    while rel not in chain:
        chain.append(rel)
        try:
            text = (root / rel).read_text()
        except OSError as exc:
            rows.append(f"{rel} cannot be read ({exc})")
            break
        scanned = _scan_js(text, rel, rows)
        if scanned is None:
            break
        try:
            data = json.loads(scanned[0])
        except ValueError as exc:
            rows.append(f"{rel} is not valid JSON after its comments are removed ({exc})")
            break
        if not isinstance(data, dict):
            rows.append(f"{rel} is not a JSON object")
            break
        ext = data.get("extends")
        if ext is None:
            break
        if not isinstance(ext, str) or not ext.startswith(("./", "../")):
            rows.append(f"{rel}: `extends` is {ext!r}; A13 follows only a relative path, so add a rule for this form on purpose (SMA-736)")
            break
        nxt = os.path.normpath(os.path.join(os.path.dirname(rel), ext))
        if not (root / nxt).is_file():
            rows.append(f"{rel}: `extends` points at {nxt}, which does not exist")
            break
        rel = nxt
    return chain


def vitest_required_inputs(root, own, closure, facts, rows):
    """Items 1 to 5 of want(T) for one vitest task (spec §4.3). Item 6 (aliases) is the caller's.

    `facts` is a list of (config, aliases, tsconfig_off), one per config file the task reads. The
    tsconfig files are skipped only when there is at least one config and every config sets
    `tsconfig: false`. With no config file, vitest runs with defaults and loads the tsconfig files.
    """
    want = {TS_LOCKFILE}
    want.update(cfg for cfg, _aliases, _off in facts)
    for name in sorted(closure):
        kind, pkg_dir = closure[name]
        data = _read_package_json(root, pkg_dir, rows)
        if data is None:
            want.add(f"{pkg_dir}/package.json")
            continue
        if kind == "workspace":
            want |= workspace_package_inputs(pkg_dir, data, rows)
        else:
            want |= _binding_runtime_inputs(root, pkg_dir, data, rows)
    if not (facts and all(off for _cfg, _aliases, off in facts)):
        for pkg_dir in [own, *sorted(d for k, d in closure.values() if k == "workspace")]:
            want.update(_tsconfig_chain(root, pkg_dir, rows))
    return want


def _builds_scratch_alias(target, blob, dest, ffi):
    """D3: True when `target` is an FFI task whose invocation names `dest`'s directory as `--out-dir`.

    The `--out-dir` value is relative to a `cd` inside the invocation, so it matches when it equals
    the directory or is a whole-segment suffix of it (`.wasmpack-test-out` matches
    `rs/crates/bindings/paigasus-wasm/.wasmpack-test-out`, `t-out` does not).
    """
    if target not in ffi:
        return False
    out_dir = os.path.dirname(dest)
    for m in _OUT_DIR_RE.finditer(blob):
        value = os.path.normpath(m.group(1).strip("'\""))
        if out_dir == value or out_dir.endswith("/" + value):
            return True
    return False


def _moon_glob_re(glob):
    """A regex for one moon input glob: `**/` and `**` cross `/`, `*` and `?` do not, `{a,b}` is a choice."""
    out, i = [], 0
    while i < len(glob):
        if glob.startswith("**/", i):
            out.append("(?:.*/)?")
            i += 3
        elif glob.startswith("**", i):
            out.append(".*")
            i += 2
        elif glob[i] == "*":
            out.append("[^/]*")
            i += 1
        elif glob[i] == "?":
            out.append("[^/]")
            i += 1
        elif glob[i] == "{" and "}" in glob[i:]:
            end = glob.index("}", i)
            out.append("(?:" + "|".join(re.escape(p) for p in glob[i + 1:end].split(",")) + ")")
            i = end + 1
        else:
            out.append(re.escape(glob[i]))
            i += 1
    return re.compile("".join(out))


def check_ts_vitest_inputs(projects, root, tracked, floor=REQUIRED_VITEST_TASKS, alias_floor=REQUIRED_VITEST_ALIASES):
    """Return the A13 violation list: vitest tasks that do not key on what vitest reads (SMA-736).

    CONTAINMENT, per task, over both input buckets, as A7 and A12a. Over-declaration is allowed. A
    declared glob that covers a wanted literal path does not satisfy it; the literal must be declared.

    `root` and `tracked` are POSITIONAL AND REQUIRED, never defaulted, for the reason in A7's
    docstring (SMA-560 I3). `tracked` is the set of git-tracked paths: an alias target that git does
    not track is valid only under D3 (`_builds_scratch_alias`).

    Row order: floor rows, then walk and parser rows, then per-task rows. A package.json walk row that
    A12a already reports is dropped here, so one broken manifest prints under one title.
    """
    tasks = derive_vitest_tasks(projects)
    ffi = derive_ffi_tasks(projects)
    a12_walk = set(ts_tsc_analysis(projects, root)[3])
    floor_rows, walk_rows, task_rows = [], [], []
    names = ts_workspace_packages(root, walk_rows)
    configs_read = set()
    task_aliases = {}
    for target in sorted(tasks):
        pid, _, task = target.partition(":")
        proj = projects[pid]
        own = proj["source_dir"]
        blob = proj["invocations"][task]
        own_pkg = _read_package_json(root, own, walk_rows)
        own_name = own_pkg.get("name") if own_pkg else None
        closure = ts_closure(root, own, names, walk_rows)
        facts = []
        for cfg in vitest_invocation_configs(target, blob, root, own, task_rows):
            try:
                text = (root / cfg).read_text()
            except OSError as exc:
                walk_rows.append(f"{cfg} cannot be read ({exc})")
                continue
            aliases, tsconfig_off = vitest_config_facts(text, cfg, walk_rows)
            facts.append((cfg, aliases, tsconfig_off))
            configs_read.add(cfg)
        task_aliases[target] = {key for _cfg, aliases, _off in facts for key, _dest in aliases}
        want = vitest_required_inputs(root, own, closure, facts, walk_rows)
        for cfg, aliases, _off in facts:
            for key, dest in aliases:
                if dest == own or dest.startswith(own + "/"):
                    continue
                if dest == ".." or dest.startswith("../") or os.path.isabs(dest):
                    task_rows.append(f"{target} aliases {key} in {cfg} to {dest}, which is outside the repository")
                    continue
                if key.startswith("@paigasus/") and key not in closure and key != own_name:
                    task_rows.append(f"{target} aliases {key} in {cfg}, but {key} is not in its package.json closure")
                    continue
                if dest in tracked:
                    want.add(dest)
                elif not _builds_scratch_alias(target, blob, dest, ffi):
                    task_rows.append(
                        f"{target} aliases {key} to the untracked path {dest}, but the task does not build it "
                        f"there (no FFI build with --out-dir {os.path.dirname(dest)})"
                    )
        files = (proj.get("task_inputs") or {}).get(task)
        globs = (proj.get("task_input_globs") or {}).get(task)
        if files is None or globs is None:
            task_rows.append(
                f"{target} reported no `inputFiles`/`inputGlobs` — moon's output shape changed, so "
                f"this assertion cannot be evaluated (treated as a violation, never skipped)"
            )
        else:
            observed = set(files) | set(globs)
            for entry in sorted(want - observed):
                task_rows.append(f"{target} inputs omit {entry}")
        deps = (proj.get("tasks") or {}).get(task) or []
        if (own_name == PROTO_PACKAGE or PROTO_PACKAGE in closure) and PROTO_GENERATE_DEP not in deps:
            task_rows.append(
                f"{target} deps omit {PROTO_GENERATE_DEP} — it reads @paigasus/proto's generated "
                f"tree, so it must run after the generator for a deterministic cache key"
            )
    if configs_read:
        smoke = projects.get(AFFECTED_SMOKE_PROJECT) or {}
        smoke_globs = (smoke.get("task_input_globs") or {}).get(AFFECTED_SMOKE_TASK)
        smoke_files = (smoke.get("task_inputs") or {}).get(AFFECTED_SMOKE_TASK) or []
        if smoke_globs is None:
            task_rows.append(
                f"{AFFECTED_SMOKE_PROJECT}:{AFFECTED_SMOKE_TASK} reported no `inputGlobs`, so A13 cannot check "
                f"that an edit to a vitest config schedules it"
            )
        else:
            patterns = [_moon_glob_re(g) for g in smoke_globs if not g.startswith("!")]
            for cfg in sorted(configs_read):
                if cfg not in smoke_files and not any(p.fullmatch(cfg) for p in patterns):
                    task_rows.append(
                        f"{cfg} matches no input of {AFFECTED_SMOKE_PROJECT}:{AFFECTED_SMOKE_TASK}, so an edit to it "
                        f"alone does not schedule the gate that reads it"
                    )
    for target in sorted(set(floor or ()) - tasks):
        floor_rows.append(
            f"FLOOR: {target} is not matched by the vitest token, so A13 asserts nothing about it "
            f"(see VITEST_TOKEN_RE; a vitest call behind a wrapper script is invisible)"
        )
    for target, keys in sorted((alias_floor or {}).items()):
        for key in sorted(set(keys) - task_aliases.get(target, set())):
            floor_rows.append(
                f"FLOOR: {target} no longer aliases {key} in a config A13 parses, so the alias rule "
                f"asserts nothing for it"
            )
    walk_rows = [row for row in walk_rows if row not in a12_walk]
    return list(dict.fromkeys(floor_rows + walk_rows + task_rows))
```

- [ ] **Step 4: Run the self-test and see the registration guard fail.**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "rc=$?"
```

Expected, and `rc=1` (the SMA-542 guard: a `check_` function that the real run does not call):

```text
  FAIL the real run never calls check_ts_vitest_inputs — a check that is defined but not invoked asserts nothing (SMA-542)
negative-control FAILED: the parity gate can pass vacuously
```

- [ ] **Step 5: Register A13.** Make these nine edits in `cargo_moon_parity.py`.

(a) The header comment. Replace:

```python
# installed-typings preflight before `tsc`. It reads package.json files from disk.
#
```

with:

```python
# installed-typings preflight before `tsc`. It reads package.json files from disk.
#
# A13 (SMA-736) is about ts vitest tasks: each one must key on its config files, its package.json
# closure, the runtime files of each `file:` binding, the tsconfig.json `extends` chains and each
# alias target outside its own package. It reads the vitest configs and tsconfig files from disk,
# and it asks git which files it tracks.
#
```

(b) `EXPECTED_FINDING_KEYS`. Replace `"a12a", "a12b")` with `"a12a", "a12b", "a13")`.

(c) The `collect_findings` signature. Replace `def collect_findings(projects, crates, root):` with `def collect_findings(projects, crates, root, tracked):`.

(d) The `collect_findings` docstring. Replace:

```python
    so `main` keeps them inside its try and maps them to rc 2.
    """
    a1, a2, a3 = check(projects, crates)
```

with:

```python
    so `main` keeps them inside its try and maps them to rc 2.

    `tracked` is the set of git-tracked paths that A13 needs (SMA-736 §4.5). `main` builds it once
    with `task_inputs.tracked_files`; the self-test passes a fixed set.
    """
    a1, a2, a3 = check(projects, crates)
```

(e) The `a13` tuple, after the `a12b` tuple. Replace:

```python
             "    A `FLOOR:` row means A12b examines less than the A12 floor — fix that first."),
    ]
```

with:

```python
             "    A `FLOOR:` row means A12b examines less than the A12 floor — fix that first."),
        ("a13", check_ts_vitest_inputs(projects, root, tracked),
             "A ts task that runs vitest does not key on a file vitest reads, so a change there\n"
             "    SELECTS NOTHING for that task and Moon serves a cached PASS (SMA-736).\n"
             "    Extra inputs are ALLOWED (this is containment, like A12a). A `FLOOR:` row, or a row\n"
             "    about a config, a tsconfig or a package.json that A13 cannot read, means the check\n"
             "    itself cannot be trusted — fix that first.\n"
             "    Fix: add the missing entry to that task's `inputs` in its own moon.yml, or to the\n"
             "    inherited `test` task in .moon/tasks/typescript-project.yml: `/ts/pnpm-lock.yaml`;\n"
             "    each vitest config; per workspace package in the closure, `/<dir>/src/**/*`,\n"
             "    `/<dir>/package.json` and each `exports` target outside src/; per `file:` binding,\n"
             "    `/<dir>/package.json` and each `files` entry that is not a `.d.ts`; each\n"
             "    tsconfig.json with its `extends` chain, unless every config sets `tsconfig: false`;\n"
             "    each tracked alias target outside the own package. A `deps omit contracts:generate`\n"
             "    row needs `deps: ['contracts:generate']` on that task. See ci/affected-graph/README.md (A13)."),
    ]
```

(f) `main` builds `tracked` inside its `try`. Replace:

```python
        crates = cargo_crates(root)
        findings = collect_findings(projects, crates, root)
```

with:

```python
        crates = cargo_crates(root)
        tracked = task_inputs.tracked_files(root)
        findings = collect_findings(projects, crates, root, tracked)
```

(g) The PASS sentence in `main`. Replace:

```python
            f"closure holds a binding"
```

with:

```python
            f"closure holds a binding, and every ts vitest task keys on its configs, its package.json "
            f"closure, its tsconfig chain and its alias targets"
```

(h) The arity call in `self_test()`. Replace:

```python
        collected = collect_findings(ok, crates, Path(tmp))
```

with:

```python
        # SMA-736: A13 needs git's tracked set. This tmp root is not a repository, so the arity
        # check passes an empty fixed set rather than calling git.
        collected = collect_findings(ok, crates, Path(tmp), frozenset())
```

(i) The OK message at the end of `self_test()`. Replace `all twelve assertions fire` with `all thirteen assertions fire`.

- [ ] **Step 6: Run the self-test and see it pass.**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "rc=$?"
```

Expected: `  OK   [parity] all thirteen assertions fire on synthetic violations` and `rc=0`.

- [ ] **Step 7: Run Ruff.** Expected: `All checks passed!`

- [ ] **Step 8: Run A13 on the real tree (M3).**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma736}"
python3 ci/affected-graph/cargo_moon_parity.py > "$SCRATCH/a13-real-task5.txt" 2>&1; echo "rc=$?"
grep -c '^      ' "$SCRATCH/a13-real-task5.txt"
grep -c 'matches no input of repo:affected-smoke' "$SCRATCH/a13-real-task5.txt"
```

Expected (measured on a scratch copy while this plan was written): `rc=1`, `78`, `13`. The only title in the file is the A13 title, which starts `A ts task that runs vitest does not key on a file vitest reads`. The 13 `matches no input` rows name the 13 tracked `vitest*.config.*` files; Task 7 clears them. The other 65 rows are the list in Task 8. If any other title appears, or the numbers differ, stop and report the file to the coordinator.

- [ ] **Step 9: Commit.**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -F- <<'EOF'
feat(ci): assert that vitest tasks key on their upstream inputs (A13) (SMA-736)

check_ts_vitest_inputs asserts, per vitest task and over both input
buckets, the lockfile, the config files, the package.json closure, the
runtime files of each file: binding, the tsconfig extends chains and each
alias target outside the own package. An untracked alias target is
valid only on an FFI task that builds it with a matching --out-dir. Two
floors and the findings key a13 stop it from passing vacuously.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

- [ ] **Step 10: Red-first proof M1 — delete the `a13` tuple.** The file is committed, so `git checkout` restores it safely. Delete the whole `("a13", check_ts_vitest_inputs(projects, root, tracked), …),` tuple (14 lines) from `collect_findings`. Then:

```bash
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma736}"
python3 ci/affected-graph/cargo_moon_parity.py --self-test > "$SCRATCH/m1.txt" 2>&1; echo "rc=$?"
cat "$SCRATCH/m1.txt"
git checkout -- ci/affected-graph/cargo_moon_parity.py
git status --short
```

Expected (measured on a scratch copy while this plan was written): `rc=1`, then exactly these four lines, then an empty `git status --short`:

```text
  FAIL the real run never calls check_ts_vitest_inputs — a check that is defined but not invoked asserts nothing (SMA-542)
  FAIL collect_findings returned 14 entries, expected 15 — a check was added or dropped without updating EXPECTED_FINDING_KEYS
  FAIL collect_findings reported ('a1', 'a2', 'a3', 'a4-lint', 'a4-fmt', 'a5', 'a6', 'a7', 'a8', 'a9', 'a10', 'a11', 'a12a', 'a12b'), expected ('a1', 'a2', 'a3', 'a4-lint', 'a4-fmt', 'a5', 'a6', 'a7', 'a8', 'a9', 'a10', 'a11', 'a12a', 'a12b', 'a13') — a check was dropped, added or reordered in the findings list
negative-control FAILED: the parity gate can pass vacuously
```

- [ ] **Step 11: Red-first proof M2 — reduce the check to `return []`.** Insert the line `    return []` as the first statement of `check_ts_vitest_inputs`, directly after its docstring. Then:

```bash
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma736}"
python3 ci/affected-graph/cargo_moon_parity.py --self-test > "$SCRATCH/m2.txt" 2>&1; echo "rc=$?"
sed -n 1,11p "$SCRATCH/m2.txt"
grep -c '^  FAIL' "$SCRATCH/m2.txt"
git checkout -- ci/affected-graph/cargo_moon_parity.py
git status --short
```

Expected (measured on a scratch copy while this plan was written): `rc=1`; the first eleven lines are the T2 rows below; the count is `33` (every A13 fixture row except T1 reds); then an empty `git status --short`.

```text
  FAIL A13 did not demand ts/pnpm-lock.yaml of c-ts:test
  FAIL A13 did not demand ts/packages/core/vitest.config.ts of c-ts:test
  FAIL A13 did not demand ts/packages/kernel/src/**/* of app-ts:test
  FAIL A13 did not demand ts/packages/proto/package.json of c-ts:test
  FAIL A13 did not demand ts/packages/core/testing/**/* of app-ts:test
  FAIL A13 did not demand rs/crates/bindings/wb/wb_bg.wasm of c-ts:test
  FAIL A13 did not demand rs/crates/bindings/wb/wb.js of c-ts:test
  FAIL A13 did not demand ts/packages/kernel/tsconfig.json of c-ts:test
  FAIL A13 did not demand ts/packages/kernel/tsconfig.json of k-ts:test
  FAIL A13 did not demand ts/tsconfig.base.json of k-ts:test
  FAIL A13 did not demand ts/packages/kernel/src/wasm.ts of c-ts:test
```

- [ ] **Step 12: Record M1, M2 and M3.** Paste the measured output of Steps 8, 10 and 11 into the Measurements section at the end of this plan, under M1, M2 and M3. Then:

```bash
git add docs/superpowers/plans/2026-10-09-sma-736-vitest-upstream-inputs.md
git commit -F- <<'EOF'
docs(ci): record the A13 red-first measurements (SMA-736)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

### Task 6: The real-corpus pin (spec §4.7, T11)

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` — self-test rows before `    for f in failures:` (after the Task 5 rows).

**Interfaces:**
- Consumes: `vitest_config_facts` (Task 2); `task_inputs.tracked_files` (Task 4).
- Produces: no new name. A literal table `a13_corpus` inside `self_test()`.

The table below is the parser's output on the 13 tracked `vitest*.config.*` files at the start of this branch, measured with the Task 2 code while this plan was written. Both app configs set `tsconfig: false`. Only the kernel config aliases a path outside its own package.

- [ ] **Step 1: Write the pin rows.** Insert this block before `    for f in failures:` (after the Task 5 rows):

```python
    # A13 (SMA-736 §4.7, T11) — the real-corpus pin. Every tracked vitest config is parsed, and the
    # result must equal this literal table of (config, sorted aliases, tsconfig_off). A parser change
    # that alters what it finds in a real config reds here, and so does a new config file: add it to
    # the table on purpose. Spec R1.
    a13_corpus = (
        ("ts/apps/gateway-console/vitest.config.ts", (
            ("next/cache", "ts/apps/gateway-console/tests/support/next-cache.ts"),
            ("next/headers", "ts/apps/gateway-console/tests/support/next-headers.ts"),
            ("server-only", "ts/apps/gateway-console/tests/support/server-only-stub.ts"),
        ), True),
        ("ts/apps/iam-console/vitest.config.ts", (
            ("next/cache", "ts/apps/iam-console/tests/support/next-cache.ts"),
            ("next/headers", "ts/apps/iam-console/tests/support/next-headers.ts"),
            ("server-only", "ts/apps/iam-console/tests/support/server-only-stub.ts"),
        ), True),
        ("ts/packages/paigasus-app-shell/vitest.config.ts", (), False),
        ("ts/packages/paigasus-auth/vitest.config.ts", (
            ("server-only", "ts/packages/paigasus-auth/tests/support/server-only-stub.ts"),
        ), False),
        ("ts/packages/paigasus-auth/vitest.containers.config.ts", (
            ("server-only", "ts/packages/paigasus-auth/tests/support/server-only-stub.ts"),
        ), False),
        ("ts/packages/paigasus-console-core/vitest.config.ts", (
            ("next/cache", "ts/packages/paigasus-console-core/tests/support/next-cache.ts"),
            ("next/headers", "ts/packages/paigasus-console-core/tests/support/next-headers.ts"),
            ("server-only", "ts/packages/paigasus-console-core/tests/support/server-only-stub.ts"),
        ), False),
        ("ts/packages/paigasus-console-core/vitest.containers.config.ts", (
            ("server-only", "ts/packages/paigasus-console-core/tests/support/server-only-stub.ts"),
        ), False),
        ("ts/packages/paigasus-discovery/vitest.config.ts", (
            ("server-only", "ts/packages/paigasus-discovery/tests/support/server-only-stub.ts"),
        ), False),
        ("ts/packages/paigasus-discovery/vitest.containers.config.ts", (
            ("server-only", "ts/packages/paigasus-discovery/tests/support/server-only-stub.ts"),
        ), False),
        ("ts/packages/paigasus-kernel/vitest.config.ts", (
            ("@paigasus/node-bindings", "rs/crates/bindings/paigasus-node-bindings/index.js"),
            ("@paigasus/wasm", "rs/crates/bindings/paigasus-wasm/.wasmpack-test-out/paigasus_wasm.js"),
        ), False),
        ("ts/packages/paigasus-next-config/vitest.config.ts", (), False),
        ("ts/packages/paigasus-sdk/vitest.config.ts", (), False),
        ("ts/packages/paigasus-ui/vitest.config.ts", (), False),
    )
    a13_corpus_root = Path(__file__).resolve().parents[2]
    a13_corpus_paths = sorted(
        p for p in task_inputs.tracked_files(a13_corpus_root)
        if re.search(r"(^|/)vitest[^/]*\.config\.[^/]+$", p)
    )
    if a13_corpus_paths != [cfg for cfg, _aliases, _off in a13_corpus]:
        failures.append(
            f"the tracked vitest configs are {a13_corpus_paths}, but the A13 corpus pin lists "
            f"{[cfg for cfg, _aliases, _off in a13_corpus]} — add a new config to the pin on purpose"
        )
    for cfg, want_aliases, want_off in a13_corpus:
        rows = []
        path = a13_corpus_root / cfg
        aliases, off = vitest_config_facts(path.read_text(), cfg, rows) if path.is_file() else ([], False)
        if rows or tuple(aliases) != want_aliases or off is not want_off:
            failures.append(
                f"the A13 parser reads {cfg} as aliases {aliases}, tsconfig_off {off}, rows {rows}; the "
                f"corpus pin says aliases {list(want_aliases)}, tsconfig_off {want_off}, no rows"
            )
```

- [ ] **Step 2: Run the self-test.**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "rc=$?"
```

Expected: `  OK   [parity] all thirteen assertions fire on synthetic violations` and `rc=0`. The parser already exists, so the pin passes on the first run. Step 4 proves that it bites.

- [ ] **Step 3: Commit.**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -F- <<'EOF'
test(ci): pin what the A13 parser reads in every real vitest config (SMA-736)

The self-test parses each tracked vitest*.config.* file and compares
the aliases and the tsconfig: false flag with a literal table. A parser
change that alters what it finds in a real config reds the self-test,
and so does a new config file.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

- [ ] **Step 4: Prove the pin bites (M4).** The file is committed, so `git checkout` restores it safely. In `vitest_config_facts`, change the last line from `return sorted(set(aliases)), bool(_JS_TSCONFIG_OFF_RE.search(masked))` to `return sorted(set(aliases)), False`. Then:

```bash
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma736}"
python3 ci/affected-graph/cargo_moon_parity.py --self-test > "$SCRATCH/m4.txt" 2>&1; echo "rc=$?"
grep -c 'the A13 parser reads' "$SCRATCH/m4.txt"
git checkout -- ci/affected-graph/cargo_moon_parity.py
git status --short
```

Expected (measured on a scratch copy): `rc=1`, then `2` (the two app configs), then an empty `git status --short`. The parser-table row `` `tsconfig: false` `` reds too. Paste the count and the two row texts under M4 in the Measurements section, and commit the plan with the message `docs(ci): record the A13 corpus pin measurement (SMA-736)` and the trailer, as in Task 5 Step 12.

---

### Task 7: Make the gate reachable from a config or tsconfig edit (spec §4.6, §4.10, T10)

**Files:**
- Modify: `moon.yml` — `repo:affected-smoke` `inputs`, after `      - 'ts/apps/*/package.json'` (line 217).
- Modify: `ci/actionlint/run.sh` — the count comment (line 2117) and `T_AFFECTED_SMOKE_REQUIRED_INPUTS` (after `  'ts/apps/*/package.json'`, line 2143).

**Interfaces:**
- Consumes: A13's reachability rows (Task 5).
- Produces: five new `repo:affected-smoke` inputs, each floored by `repo:actionlint` check 8e.

- [ ] **Step 1: See the reachability rows (red).** Task 5 Step 8 saved them:

```bash
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma736}"
grep 'matches no input of repo:affected-smoke' "$SCRATCH/a13-real-task5.txt"
```

Expected: 13 rows, one per tracked `vitest*.config.*` file, for example `ts/packages/paigasus-kernel/vitest.config.ts matches no input of repo:affected-smoke, so an edit to it alone does not schedule the gate that reads it`.

- [ ] **Step 2: Add the inputs to `moon.yml`.** In the `affected-smoke:` task, after the line `      - 'ts/apps/*/package.json'` (line 217; it is the second of the two such lines in the file), insert:

```yaml
      # SMA-736 — A13 READS every vitest config and every tsconfig.json `extends` chain from disk,
      # so an edit to one of these files alone must schedule this gate. Without these globs the
      # assertion is real but unreachable on exactly the PR that adds an alias or moves a config.
      # Plain globs, never a brace pattern: ci/affected-graph/task_inputs.py rejects braces.
      - 'ts/packages/*/vitest*.config.*'
      - 'ts/apps/*/vitest*.config.*'
      - 'ts/packages/*/tsconfig*.json'
      - 'ts/apps/*/tsconfig*.json'
      - 'ts/tsconfig.base.json'
```

- [ ] **Step 3: Floor them in `ci/actionlint/run.sh`.** In `T_AFFECTED_SMOKE_REQUIRED_INPUTS`, after the line `  'ts/apps/*/package.json'` (line 2143), insert:

```bash
  # SMA-736 — floors the inputs that make A13 reachable. Without them, a PR that edits only a
  # vitest config or a tsconfig.json does not schedule repo:affected-smoke, and A13 cannot fire.
  'ts/packages/*/vitest*.config.*'
  'ts/apps/*/vitest*.config.*'
  'ts/packages/*/tsconfig*.json'
  'ts/apps/*/tsconfig*.json'
  'ts/tsconfig.base.json'
```

Then count the entries:

```bash
sed -n '/^T_AFFECTED_SMOKE_REQUIRED_INPUTS=(/,/^)/p' ci/actionlint/run.sh | grep -c "^  '"
```

Expected: `34`. In the comment at line 2117, replace `the list is twenty-three entries` with `the list is thirty-four entries` (the old number was already stale: the list held 29 entries before this task).

- [ ] **Step 4: Check moon's resolved view.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma736}"
moon query projects > "$SCRATCH/projects.json"; echo "rc=$?"
python3 -c 'import json,sys; t=next(p for p in json.load(open(sys.argv[1]))["projects"] if p["id"]=="repo")["tasks"]["affected-smoke"]; print(sorted(t["inputGlobs"])); print(sorted(t["inputFiles"]))' "$SCRATCH/projects.json"
```

Expected: `rc=0`; the globs list holds the four new globs; the files list holds `ts/tsconfig.base.json`.

- [ ] **Step 5: Run A13 on the real tree.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma736}"
python3 ci/affected-graph/cargo_moon_parity.py > "$SCRATCH/a13-real-task7.txt" 2>&1; echo "rc=$?"
grep -c '^      ' "$SCRATCH/a13-real-task7.txt"
grep -c 'matches no input' "$SCRATCH/a13-real-task7.txt"
```

Expected: `rc=1`, `65`, `0`. The 65 rows are the list in Task 8.

- [ ] **Step 6: Run `repo:input-liveness`.** It asserts that each new glob matches tracked files.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
python3 ci/affected-graph/task_inputs.py; echo "rc=$?"
```

Expected: `rc=0`.

- [ ] **Step 7: Run `repo:actionlint`.**

```bash
/opt/homebrew/bin/bash ci/actionlint/run.sh; echo "rc=$?"
```

Read the pipe preflight line first. If it reports a pipe capacity of 8192 bytes or more, expected: `rc=0` with no `missing-input` row. If it reports a small pipe and exits rc 2, the host is in the 512-byte-pipe state (SMA-612). Do not try another bash. Tell the coordinator, and rely on `repo:actionlint` in CI.

- [ ] **Step 8: Commit.**

```bash
git add moon.yml ci/actionlint/run.sh
git commit -F- <<'EOF'
feat(ci): schedule repo:affected-smoke on vitest config and tsconfig edits (SMA-736)

A13 reads the vitest configs and the tsconfig extends chains from disk.
An edit to one of those files alone must schedule the gate, so the task
now keys on them, and repo:actionlint check 8e floors the five globs.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

### Task 8: Fix the real inputs and deps (spec §4.11)

**Files:**
- Modify: `.moon/tasks/typescript-project.yml` (the `test` task, lines 46-52)
- Modify: `ts/apps/gateway-console/moon.yml` (`test` inputs, line 351)
- Modify: `ts/apps/iam-console/moon.yml` (`test` inputs, line 384)
- Modify: `ts/packages/paigasus-app-shell/moon.yml` (`test`, lines 62-78)
- Modify: `ts/packages/paigasus-auth/moon.yml` (`test-e2e` inputs, line 52)
- Modify: `ts/packages/paigasus-console-core/moon.yml` (`test` inputs, line 125; `test-e2e` inputs, line 167)
- Modify: `ts/packages/paigasus-discovery/moon.yml` (`test` inputs, line 59; `test-e2e` inputs, line 84)
- Modify: `ts/packages/paigasus-sdk/moon.yml` (`test` inputs, line 69)

**Interfaces:**
- Consumes: the A13 rows of the real run (Task 7 Step 5).
- Produces: inputs and one dep. No product code changes.

These are the 65 rows that A13 printed on the real tree while this plan was written (Task 7 Step 5 must give the same list). They are the source of truth only after you compare them with your own run. Use your own output. If a row in your output is not in this list, or a row looks wrong rather than missing, stop and ask the coordinator.

```text
gateway-console-ts:test inputs omit rs/crates/bindings/paigasus-node-bindings/index.js
gateway-console-ts:test inputs omit rs/crates/bindings/paigasus-node-bindings/package.json
iam-console-ts:test inputs omit rs/crates/bindings/paigasus-node-bindings/index.js
iam-console-ts:test inputs omit rs/crates/bindings/paigasus-node-bindings/package.json
paigasus-app-shell-ts:test inputs omit ts/packages/paigasus-app-shell/tsconfig.json
paigasus-app-shell-ts:test inputs omit ts/packages/paigasus-auth/tsconfig.json
paigasus-app-shell-ts:test inputs omit ts/packages/paigasus-discovery/tsconfig.json
paigasus-app-shell-ts:test inputs omit ts/packages/paigasus-next-config/package.json
paigasus-app-shell-ts:test inputs omit ts/packages/paigasus-next-config/src/**/*
paigasus-app-shell-ts:test inputs omit ts/packages/paigasus-next-config/tsconfig.app.json
paigasus-app-shell-ts:test inputs omit ts/packages/paigasus-next-config/tsconfig.json
paigasus-app-shell-ts:test inputs omit ts/packages/paigasus-proto/package.json
paigasus-app-shell-ts:test inputs omit ts/packages/paigasus-proto/src/**/*
paigasus-app-shell-ts:test inputs omit ts/packages/paigasus-proto/tsconfig.json
paigasus-app-shell-ts:test inputs omit ts/packages/paigasus-ui/tsconfig.json
paigasus-app-shell-ts:test inputs omit ts/tsconfig.base.json
paigasus-app-shell-ts:test deps omit contracts:generate — it reads @paigasus/proto's generated tree, so it must run after the generator for a deterministic cache key
paigasus-auth-ts:test inputs omit ts/packages/paigasus-auth/tsconfig.json
paigasus-auth-ts:test inputs omit ts/tsconfig.base.json
paigasus-auth-ts:test-e2e inputs omit ts/packages/paigasus-auth/tsconfig.json
paigasus-auth-ts:test-e2e inputs omit ts/tsconfig.base.json
paigasus-console-core-ts:test inputs omit rs/crates/bindings/paigasus-node-bindings/index.js
paigasus-console-core-ts:test inputs omit rs/crates/bindings/paigasus-node-bindings/package.json
paigasus-console-core-ts:test inputs omit ts/packages/paigasus-auth/tsconfig.json
paigasus-console-core-ts:test inputs omit ts/packages/paigasus-console-core/tsconfig.json
paigasus-console-core-ts:test inputs omit ts/packages/paigasus-discovery/tsconfig.json
paigasus-console-core-ts:test inputs omit ts/packages/paigasus-kernel/tsconfig.json
paigasus-console-core-ts:test inputs omit ts/packages/paigasus-proto/tsconfig.json
paigasus-console-core-ts:test inputs omit ts/packages/paigasus-sdk/tsconfig.json
paigasus-console-core-ts:test inputs omit ts/tsconfig.base.json
paigasus-console-core-ts:test-e2e inputs omit rs/crates/bindings/paigasus-node-bindings/index.js
paigasus-console-core-ts:test-e2e inputs omit rs/crates/bindings/paigasus-node-bindings/package.json
paigasus-console-core-ts:test-e2e inputs omit ts/packages/paigasus-auth/package.json
paigasus-console-core-ts:test-e2e inputs omit ts/packages/paigasus-auth/src/**/*
paigasus-console-core-ts:test-e2e inputs omit ts/packages/paigasus-auth/tsconfig.json
paigasus-console-core-ts:test-e2e inputs omit ts/packages/paigasus-discovery/tsconfig.json
paigasus-console-core-ts:test-e2e inputs omit ts/packages/paigasus-kernel/tsconfig.json
paigasus-console-core-ts:test-e2e inputs omit ts/packages/paigasus-proto/package.json
paigasus-console-core-ts:test-e2e inputs omit ts/packages/paigasus-proto/src/**/*
paigasus-console-core-ts:test-e2e inputs omit ts/packages/paigasus-proto/tsconfig.json
paigasus-console-core-ts:test-e2e inputs omit ts/packages/paigasus-sdk/package.json
paigasus-console-core-ts:test-e2e inputs omit ts/packages/paigasus-sdk/src/**/*
paigasus-console-core-ts:test-e2e inputs omit ts/packages/paigasus-sdk/tsconfig.json
paigasus-console-core-ts:test-e2e inputs omit ts/tsconfig.base.json
paigasus-discovery-ts:test inputs omit ts/packages/paigasus-discovery/tsconfig.json
paigasus-discovery-ts:test inputs omit ts/packages/paigasus-proto/package.json
paigasus-discovery-ts:test inputs omit ts/packages/paigasus-proto/tsconfig.json
paigasus-discovery-ts:test inputs omit ts/tsconfig.base.json
paigasus-discovery-ts:test-e2e inputs omit ts/packages/paigasus-discovery/tsconfig.json
paigasus-discovery-ts:test-e2e inputs omit ts/packages/paigasus-proto/package.json
paigasus-discovery-ts:test-e2e inputs omit ts/packages/paigasus-proto/src/**/*
paigasus-discovery-ts:test-e2e inputs omit ts/packages/paigasus-proto/tsconfig.json
paigasus-discovery-ts:test-e2e inputs omit ts/tsconfig.base.json
paigasus-kernel-ts:test inputs omit ts/packages/paigasus-kernel/tsconfig.json
paigasus-kernel-ts:test inputs omit ts/tsconfig.base.json
paigasus-next-config-ts:test inputs omit ts/packages/paigasus-next-config/tsconfig.json
paigasus-next-config-ts:test inputs omit ts/tsconfig.base.json
paigasus-proto-ts:test inputs omit ts/packages/paigasus-proto/tsconfig.json
paigasus-proto-ts:test inputs omit ts/tsconfig.base.json
paigasus-sdk-ts:test inputs omit ts/packages/paigasus-proto/package.json
paigasus-sdk-ts:test inputs omit ts/packages/paigasus-proto/tsconfig.json
paigasus-sdk-ts:test inputs omit ts/packages/paigasus-sdk/tsconfig.json
paigasus-sdk-ts:test inputs omit ts/tsconfig.base.json
paigasus-ui-ts:test inputs omit ts/packages/paigasus-ui/tsconfig.json
paigasus-ui-ts:test inputs omit ts/tsconfig.base.json
```

The edits below clear every row. They were checked against a simulation of moon's view while this plan was written, not against moon itself; Step 11 checks them against moon.

- [ ] **Step 1: Compare the row lists.**

```bash
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma736}"
sed -n 's/^      //p' "$SCRATCH/a13-real-task7.txt" > "$SCRATCH/a13-rows.txt"
wc -l < "$SCRATCH/a13-rows.txt"
```

Expected: `65`. Read the file. It must equal the list above. If not, stop and ask.

- [ ] **Step 2: The inherited `test` task.** In `.moon/tasks/typescript-project.yml`, in the `test:` task, after its `      - '/ts/pnpm-lock.yaml'` line (the last line of the file), add:

```yaml
      # SMA-736 (A13). vite's oxc transform loads the nearest tsconfig.json of each file it
      # transforms, and that file extends the base, so both change what vitest runs.
      - 'tsconfig.json'
      - '/ts/tsconfig.base.json'
```

This clears the own-package `tsconfig.json` row and the `ts/tsconfig.base.json` row of every `test` task that merges the inherited inputs: app-shell, auth, console-core, discovery, kernel, next-config, proto, sdk and ui. The two app `test` tasks use `merge: replace` and already list both files.

- [ ] **Step 3: The two apps.** In `ts/apps/gateway-console/moon.yml` (line 351) and in `ts/apps/iam-console/moon.yml` (line 384), in the `test:` task, after the line `      - '/rs/crates/bindings/paigasus-wasm/package.json'` that comes directly before `    options:` / `      merge: replace`, add:

```yaml
      # SMA-736 (A13). The kernel's package.json closure holds the napi binding, so every vitest
      # task that reaches the kernel keys on the binding's runtime files (spec D6, R2).
      - '/rs/crates/bindings/paigasus-node-bindings/index.js'
      - '/rs/crates/bindings/paigasus-node-bindings/package.json'
```

- [ ] **Step 4: app-shell `test`.** In `ts/packages/paigasus-app-shell/moon.yml`, replace the line `  test:` (line 62, the first `test:` in the file) with:

```yaml
  test:
    # SMA-736 (A13): @paigasus/proto is in this package's closure (through discovery), so the task
    # must run after the generator for a deterministic cache key.
    deps: ['contracts:generate']
```

Then, in the same task, after the line `      - '/ts/packages/paigasus-discovery/package.json'` (line 76), add:

```yaml
      # SMA-736 (A13): the rest of the package.json closure, and the tsconfig chain of each
      # workspace package in it. next-config is a direct dependency; proto comes through discovery.
      - '/ts/packages/paigasus-next-config/src/**/*'
      - '/ts/packages/paigasus-next-config/package.json'
      - '/ts/packages/paigasus-next-config/tsconfig.app.json'
      - '/ts/packages/paigasus-next-config/tsconfig.json'
      - '/ts/packages/paigasus-proto/src/**/*'
      - '/ts/packages/paigasus-proto/package.json'
      - '/ts/packages/paigasus-proto/tsconfig.json'
      - '/ts/packages/paigasus-ui/tsconfig.json'
      - '/ts/packages/paigasus-auth/tsconfig.json'
      - '/ts/packages/paigasus-discovery/tsconfig.json'
```

- [ ] **Step 5: auth `test-e2e`.** In `ts/packages/paigasus-auth/moon.yml`, in the `test-e2e:` task, after its `      - '/ts/pnpm-lock.yaml'` line (line 52, the second such line in the file), add:

```yaml
      # SMA-736 (A13): vite's oxc transform reads this package's tsconfig chain.
      - 'tsconfig.json'
      - '/ts/tsconfig.base.json'
```

- [ ] **Step 6: console-core `test`.** In `ts/packages/paigasus-console-core/moon.yml`, in the `test:` task, after the line `      - '/ts/apps/*/package.json'` (line 125), add:

```yaml
      # SMA-736 (A13): the napi binding's runtime files (through the kernel) and the tsconfig chain
      # of each workspace package in the closure.
      - '/rs/crates/bindings/paigasus-node-bindings/index.js'
      - '/rs/crates/bindings/paigasus-node-bindings/package.json'
      - '/ts/packages/paigasus-auth/tsconfig.json'
      - '/ts/packages/paigasus-discovery/tsconfig.json'
      - '/ts/packages/paigasus-kernel/tsconfig.json'
      - '/ts/packages/paigasus-proto/tsconfig.json'
      - '/ts/packages/paigasus-sdk/tsconfig.json'
```

- [ ] **Step 7: console-core `test-e2e`.** In the same file, in the `test-e2e:` task, after the line `      - '/rs/crates/bindings/paigasus-wasm/package.json'` (line 167, the last such line in the file, directly before `    options:`), add:

```yaml
      # SMA-736 (A13): the rest of the package.json closure, the napi binding's runtime files and
      # the tsconfig chain. So an auth, sdk or proto edit now selects this Docker tier (spec R2,
      # accepted in spec §9).
      - '/ts/tsconfig.base.json'
      - '/ts/packages/paigasus-auth/src/**/*'
      - '/ts/packages/paigasus-auth/package.json'
      - '/ts/packages/paigasus-auth/tsconfig.json'
      - '/ts/packages/paigasus-sdk/src/**/*'
      - '/ts/packages/paigasus-sdk/package.json'
      - '/ts/packages/paigasus-sdk/tsconfig.json'
      - '/ts/packages/paigasus-proto/src/**/*'
      - '/ts/packages/paigasus-proto/package.json'
      - '/ts/packages/paigasus-proto/tsconfig.json'
      - '/ts/packages/paigasus-discovery/tsconfig.json'
      - '/ts/packages/paigasus-kernel/tsconfig.json'
      - '/rs/crates/bindings/paigasus-node-bindings/index.js'
      - '/rs/crates/bindings/paigasus-node-bindings/package.json'
```

- [ ] **Step 8: discovery `test` and `test-e2e`.** In `ts/packages/paigasus-discovery/moon.yml`, in the `test:` task, after the line `      - 'README.md'` (line 59), add:

```yaml
      # SMA-736 (A13): the rest of @paigasus/proto's input set.
      - '/ts/packages/paigasus-proto/package.json'
      - '/ts/packages/paigasus-proto/tsconfig.json'
```

In the `test-e2e:` task, after the line `      - '/ts/pnpm-lock.yaml'` (line 84), add:

```yaml
      # SMA-736 (A13): @paigasus/proto (the closure) and the tsconfig chain.
      - 'tsconfig.json'
      - '/ts/tsconfig.base.json'
      - '/ts/packages/paigasus-proto/src/**/*'
      - '/ts/packages/paigasus-proto/package.json'
      - '/ts/packages/paigasus-proto/tsconfig.json'
```

- [ ] **Step 9: sdk `test`.** In `ts/packages/paigasus-sdk/moon.yml`, in the `test:` task, after the line `      - '/rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs'` (line 69, the last line of the file), add:

```yaml
      # SMA-736 (A13): the rest of @paigasus/proto's input set.
      - '/ts/packages/paigasus-proto/package.json'
      - '/ts/packages/paigasus-proto/tsconfig.json'
```

- [ ] **Step 10: Check the YAML format.** `ts:fmt` runs `prettier --check .` over `ts/`, which includes the `moon.yml` files.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
moon run ts:fmt --force; echo "rc=$?"
```

Expected: `rc=0`. If `ts/node_modules` is empty, run `pnpm -C ts install` first. If Prettier reports a file, run `pnpm -C ts exec prettier --write <file>` on that file and look at the diff.

- [ ] **Step 11: Run A13 on the real tree and see it pass.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
python3 ci/affected-graph/cargo_moon_parity.py; echo "rc=$?"
```

Expected: one line that starts `PASS  cargo-moon-parity` and ends `and every ts vitest task keys on its configs, its package.json closure, its tsconfig chain and its alias targets`, and `rc=0`. If a row is left, read it, and fix the owning `moon.yml` only when the row is a missing input. If it looks wrong, stop and ask.

- [ ] **Step 12: Check one task with no tsconfig.json.** The inherited `tsconfig.json` input names a file that `ts/packages/commitlint-config` does not have.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
moon run commitlint-config-ts:test --force; echo "rc=$?"
```

Expected: `rc=0`.

- [ ] **Step 13: Commit.**

```bash
git add .moon/tasks/typescript-project.yml ts/apps/gateway-console/moon.yml ts/apps/iam-console/moon.yml \
  ts/packages/paigasus-app-shell/moon.yml ts/packages/paigasus-auth/moon.yml \
  ts/packages/paigasus-console-core/moon.yml ts/packages/paigasus-discovery/moon.yml ts/packages/paigasus-sdk/moon.yml
git commit -F- <<'EOF'
fix(ts): key every vitest task on the upstream files that vitest reads (SMA-736)

A13 found vitest tasks that did not declare a file vitest reads: the
napi glue through the kernel, the tsconfig extends chains, and parts of
the package.json closure. app-shell's test reads @paigasus/proto and now
runs after contracts:generate. No product code changes.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

### Task 9: Re-baseline `ci/affected-graph/run.sh` by measurement (spec §4.12)

**Files:**
- Modify: `ci/affected-graph/run.sh` — the `napi-glue-js->kernel-test` case and its comment (lines 763-770), and every case CSV that the measurement changes.
- Create (not committed): `$SCRATCH/rebaseline.py`.
- Modify: this plan — the Measurements section (M5).

**Interfaces:**
- Consumes: the Task 8 inputs.
- Produces: the case `napi-glue-js->vitest`; re-measured expected CSVs.

- [ ] **Step 1: See the red.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma736}"
/bin/bash ci/affected-graph/run.sh > "$SCRATCH/run-sh-before.txt" 2>&1; echo "rc=$?"
grep '^FAIL' "$SCRATCH/run-sh-before.txt"
```

Expected: `rc=1`, with `FAIL  [<label>] affected TASK set != expected set` rows. The cases that this plan predicts to change are listed in Step 5.

- [ ] **Step 2: Rename the control case.** In `ci/affected-graph/run.sh`, replace `run_task_case_ci "napi-glue-js->kernel-test"` with `run_task_case_ci "napi-glue-js->vitest"`. Then replace the comment line

```bash
  # `moon query tasks --affected` traversal `_assert_task_case_impl` uses.
  run_task_case_ci "napi-glue-js->vitest" "rs/crates/bindings/paigasus-node-bindings/index.js" \
```

with:

```bash
  # `moon query tasks --affected` traversal `_assert_task_case_impl` uses.
  # SMA-736: A13 makes every vitest task whose package closure holds the napi binding key on
  # index.js, so a glue-only edit now also selects the console-core and app `test` tasks and
  # console-core's `test-e2e` (spec D6, R2). This case is A13's behavioural control. It was named
  # `napi-glue-js->kernel-test` until SMA-736.
  run_task_case_ci "napi-glue-js->vitest" "rs/crates/bindings/paigasus-node-bindings/index.js" \
```

- [ ] **Step 3: Write the re-baseline helper.** Write `$SCRATCH/rebaseline.py` (not committed). It takes one target per `tasks[project][task]` from the JSON. It never greps `"target"`, which counts scheduled `deps` as selections.

```python
# usage, from the worktree root: python3 "$SCRATCH/rebaseline.py" [--write]
# Re-measures every run_case, run_task_case and run_task_case_ci in ci/affected-graph/run.sh with the
# same moon query each helper uses. Prints ADDED and REMOVED per case. With --write, appends each
# case's ADDED entries to its CSV. Refuses to write when any case LOST an entry.
import json
import re
import subprocess
import sys
from pathlib import Path

NAMES = ("build", "test", "lint", "test-e2e", "typecheck")
path = Path("ci/affected-graph/run.sh")
text = path.read_text()
CASE_RE = re.compile(r'((run_case|run_task_case|run_task_case_ci)\s+"([^"]+)"\s+"([^"]+)"\s*\\\n\s*")([^"]*)(")')
lost = []


def measure(fn, touched):
    if fn == "run_case":
        out = subprocess.run(
            ["moon", "query", "projects", "--affected", "--downstream", "deep"],
            input=touched + "\n", capture_output=True, text=True, check=True,
        ).stdout
        return sorted(p["id"] for p in json.loads(out)["projects"] if p["id"] != "repo")
    flags = ["--downstream", "deep"] if fn == "run_task_case" else []
    out = subprocess.run(
        ["moon", "query", "tasks", "--affected", *flags],
        input=touched + "\n", capture_output=True, text=True, check=True,
    ).stdout
    tasks = json.loads(out).get("tasks") or {}
    return sorted(f"{pid}:{name}" for pid, names in tasks.items() for name in names if name in NAMES)


def rewrite(m):
    fn, label, touched, csv = m.group(2), m.group(3), m.group(4), m.group(5)
    got = measure(fn, touched)
    want = csv.split(",")
    added = [t for t in got if t not in want]
    removed = [t for t in want if t not in got]
    line = text.count("\n", 0, m.start()) + 1
    print(f"{line}\t{label}\tADDED={','.join(added) or '-'}\tREMOVED={','.join(removed) or '-'}")
    if removed:
        lost.append(label)
    return m.group(1) + ",".join(want + added) + m.group(6)


new_text = CASE_RE.sub(rewrite, text)
if lost:
    sys.exit(f"rebaseline: these cases LOST an entry: {', '.join(lost)}. Stop. Nothing was written.")
if "--write" in sys.argv[1:]:
    path.write_text(new_text)
    print("rebaseline: run.sh rewritten")
```

While this plan was written, this helper ran on the unchanged tree and printed 42 lines, each `ADDED=-	REMOVED=-`.

- [ ] **Step 4: Measure.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma736}"
python3 "$SCRATCH/rebaseline.py" > "$SCRATCH/rebaseline.txt"; echo "rc=$?"
grep -v 'ADDED=-	REMOVED=-' "$SCRATCH/rebaseline.txt"
```

Expected: `rc=0`, 42 lines in `rebaseline.txt`, and every `REMOVED=-`.

- [ ] **Step 5: Check the additions before you write.** The rules:
  - Each `ADDED` entry must be a `:test` or `:test-e2e` target. Task 8 added inputs only to `test` and `test-e2e` tasks, and a dep never selects a task.
  - A `run_case` (project) line must show `ADDED=-`.
  - The plan predicts these additions, from the Task 8 inputs and moon's glob rules (not measured):

```text
auth->auth-tasks                 paigasus-console-core-ts:test-e2e
proto->sdk                       paigasus-app-shell-ts:test,paigasus-console-core-ts:test-e2e,paigasus-discovery-ts:test-e2e
proto-iam->sdk                   paigasus-app-shell-ts:test,paigasus-console-core-ts:test-e2e,paigasus-discovery-ts:test-e2e
sdk->iam-console                 paigasus-console-core-ts:test-e2e
sdk-errors->iam-console          paigasus-console-core-ts:test-e2e
napi-glue-js->vitest             gateway-console-ts:test,iam-console-ts:test,paigasus-console-core-ts:test,paigasus-console-core-ts:test-e2e
```

  `napi-glue-dts->kernel-test` is predicted NOT to change: A13 does not demand a `.d.ts`, so Task 8 added no `index.d.ts` input. Spec §4.12 expected it to change; measure it, and do not force it.

  A measured addition that differs from the prediction but obeys the first two rules is allowed: the measurement wins (spec §4.12). Record the difference in M5. Any `REMOVED` entry, any target that is not `:test` or `:test-e2e`, or any `run_case` addition is a stop: report the line from `rebaseline.txt` to the coordinator. Do not write.

- [ ] **Step 6: Write and re-measure.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma736}"
python3 "$SCRATCH/rebaseline.py" --write
python3 "$SCRATCH/rebaseline.py" | grep -v 'ADDED=-	REMOVED=-'; echo "grep rc=$? (1 = every case exact)"
git diff --stat ci/affected-graph/run.sh
```

Expected: `rebaseline: run.sh rewritten`, then `grep rc=1`.

- [ ] **Step 7: Run the guard, both modes.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma736}"
/bin/bash ci/affected-graph/run.sh --negative-control; echo "negative-control rc=$?"
/bin/bash ci/affected-graph/run.sh > "$SCRATCH/run-sh-after.txt" 2>&1; echo "rc=$?"
grep -c '^PASS' "$SCRATCH/run-sh-after.txt"; grep '^FAIL' "$SCRATCH/run-sh-after.txt"
```

Expected: `negative-control rc=0`; `rc=0`; no `FAIL` line; the `PASS  napi-glue-js->vitest` line is present.

- [ ] **Step 8: Run the Moon task as CI does.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma736}"
PATH="$SCRATCH/bashshim:$PATH" moon run repo:affected-smoke --force; echo "rc=$?"
```

Expected: `rc=0`. A failure under about 3 s that names `proto-shim` is the SMA-592 shim race (ci/CLAUDE.md), not a finding: capture the output, then run it once more.

- [ ] **Step 9: Record M5.** Paste `$SCRATCH/rebaseline.txt` (the Step 4 output) under M5 in the Measurements section, with a note for any difference from the Step 5 prediction.

- [ ] **Step 10: Commit.**

```bash
git add ci/affected-graph/run.sh docs/superpowers/plans/2026-10-09-sma-736-vitest-upstream-inputs.md
git commit -F- <<'EOF'
test(ci): re-baseline the affected-graph cases for the A13 inputs (SMA-736)

The new vitest inputs make some edits select more test and test-e2e
tasks. Each expected set is measured with the case's own moon query, not
derived. The napi glue case is renamed napi-glue-js->vitest; it is the
behavioural control for A13.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

### Task 10: Documents (spec §6)

**Files:**
- Modify: `ci/affected-graph/README.md` — lines 312-313 (the A12 "Limits" sentence) and a new A13 bullet after the A12 bullet (after line 316).
- Modify: `ci/CLAUDE.md` — the rule at lines 64-67.

**Interfaces:**
- Consumes: the final names from Tasks 1-9.
- Produces: documentation only.

- [ ] **Step 1: Replace the out-of-scope sentence.** In `ci/affected-graph/README.md`, replace:

```text
  Limits: vitest `test` tasks are out of scope, because they resolve the bindings through
  `vitest.config.ts` aliases (a follow-up issue holds them); `next build`'s own type check is not a
```

with:

```text
  Limits: vitest tasks are not A12's; A13 (below) covers them. `next build`'s own type check is not a
```

- [ ] **Step 2: Add the A13 bullet.** Directly after the A12 bullet (it ends with `script is invisible (the floor catches only the loss of a known task).`), insert:

```text
- **A13** (`check_ts_vitest_inputs` in `cargo_moon_parity.py`, SMA-736, findings key `a13`) covers
  every task of a `language: typescript` project whose resolved invocation runs vitest
  (`VITEST_TOKEN_RE`), `test` and `test-e2e` alike. For each vitest call it finds the config files:
  `--config`, `--config=` or `-c`, else vitest's own lookup (`vitest.config.*`, then
  `vite.config.*`) in the task's directory. A task with no config file runs with vitest's defaults;
  that is not a row. A13 asserts CONTAINMENT, per task and over both input buckets, like A12a. A
  task must declare `ts/pnpm-lock.yaml` and each config file. For each workspace package in A12's
  `package.json` closure, it must declare the A12 set, from the shared `workspace_package_inputs`.
  For each `file:` binding, it must declare `package.json` and each `files` entry that is not a
  `.d.ts`: vitest runs the glue and the `.wasm`, and never reads a typing. It must declare the
  `tsconfig.json` of its own package and of each workspace package, with each file of the
  relative `extends` chain, because vite's oxc transform loads the nearest tsconfig. Only a task
  whose every config sets `tsconfig: false` (the two apps) skips them. A13 parses each config with a
  narrow single-pass scanner (`vitest_config_facts`), and demands each tracked alias target outside
  the own package. An untracked alias target is valid only on an FFI task (A5's
  `derive_ffi_tasks`) whose invocation builds it with a matching `--out-dir` (the kernel's
  `.wasmpack-test-out`). A task that reads `@paigasus/proto` needs a direct `contracts:generate`
  dep. These are rows, never skips: a config split across files (a relative import, `mergeConfig`,
  `extends`); a string `projects` entry; an alias form outside the grammar; `--root`; a `cd` before
  the vitest call outside a subshell; an `extends` that is not a relative path; a binding with no
  `files` list; a config that no input of `repo:affected-smoke` matches. The floors are
  `REQUIRED_VITEST_TASKS`, `REQUIRED_VITEST_ALIASES` and a self-test pin of what the parser reads
  in every tracked `vitest*.config.*` file. A `package.json` walk row that A12a prints is not
  printed again. Accepted over-approximation: the rule is per package, not per imported file, so
  the console tests key on the napi glue that they never load, and an upstream source edit selects
  more `test-e2e` tasks (spec D6, R2). Limits: own-package files other than the config and
  `tsconfig.json` (`tests/**`, `setupFiles`, in-package aliases) stay with the hand-written lists;
  a vitest call behind a wrapper script is invisible; a stale pnpm-installed binding copy on a
  developer host is not seen (CI installs fresh); a regex literal that holds a quote reds the
  parser rather than being read.
```

- [ ] **Step 3: Extend the rule in `ci/CLAUDE.md`.** Replace:

```text
- A new ts workspace dependency needs matching inputs on every `tsc` task of the package and of
  its dependents. A new `file:` binding also needs the preflight
  `node ../../../ts/scripts/check-installed-bindings.mjs &&` before `tsc`. If either is missing,
  A12 in `repo:affected-smoke` reds (SMA-536, `ci/affected-graph/README.md`).
```

with:

```text
- A new ts workspace dependency needs matching inputs on every `tsc` task of the package and of
  its dependents. A new `file:` binding also needs the preflight
  `node ../../../ts/scripts/check-installed-bindings.mjs &&` before `tsc`. If either is missing,
  A12 in `repo:affected-smoke` reds (SMA-536, `ci/affected-graph/README.md`).
  The same dependency also needs matching inputs on every vitest task (`test`, and a `test-e2e`
  that runs vitest) of the package and of its dependents. That is the A12 set, plus each `files`
  entry of a binding that is not a `.d.ts`, plus the `tsconfig.json` of each workspace package with
  its `extends` chain. A new vitest config needs its own file as an input, and a new alias needs
  its tracked target. If one is missing, A13 reds (SMA-736). A config that no input of
  `repo:affected-smoke` matches reds A13 too: add its glob to `moon.yml` and to
  `T_AFFECTED_SMOKE_REQUIRED_INPUTS` in `ci/actionlint/run.sh`.
```

- [ ] **Step 4: Check the documents.**

```bash
git diff -U0 -- ci/affected-graph/README.md ci/CLAUDE.md | grep -ci 'cirep[o]rt'
grep -n 'A13' ci/affected-graph/README.md ci/CLAUDE.md
```

Expected: the first command prints `0` (no added line names the moon report file). The second shows the new bullet, the changed Limits sentence and the rule.

- [ ] **Step 5: Commit.**

```bash
git add ci/affected-graph/README.md ci/CLAUDE.md
git commit -F- <<'EOF'
docs(ci): document A13 and the vitest input rule (SMA-736)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

## Self-review notes

### Spec coverage

| Spec section | Task |
|---|---|
| §1 Problem; §2 items 1-5 (what vitest reads) | 5 (required set), 2 (aliases), 3 (lookup) |
| §3 D1 scope (12 `test` + 3 `test-e2e`) | 3 (`derive_vitest_tasks`), 5 (floor), 8 |
| §3 D2 static parse, fail closed | 2 |
| §3 D3 scratch alias (FFI task + `--out-dir`) | 5 (`_builds_scratch_alias`, T3) |
| §3 D4 separate key `a13` | 5 |
| §3 D5 containment, literal paths | 5 (T2 alias target row) |
| §3 D6 package granularity | 5, 8, 9 |
| §4.1 task selection | 3 |
| §4.2 config files | 3 |
| §4.3 items 1-7 required set | 1 (item 3 helper), 5 |
| §4.4 config parser steps 1-8 | 2 |
| §4.5 tracked-file check | 4, 5 |
| §4.6 gate reachability row | 5 (T10), 7 |
| §4.7 floors and the real-corpus pin | 5, 6 |
| §4.8 function and row shape; walk rows once | 5 (T12) |
| §4.9 registration, PASS sentence, header, counts | 5 (`cargo_moon_parity.py`), 7 (`ci/actionlint/run.sh:2117`) |
| §4.10 reachability of the gate | 7 |
| §4.11 input and dep fixes | 8 |
| §4.12 `run.sh` re-baseline and rename | 9 |
| §5 T1-T4, T7-T10, T12 | 5 (T12 first half: 1) |
| §5 T5 | 2 |
| §5 T6 | 3 |
| §5 T11 | 6 |
| §5 red-first proof (arity, `return []`) | 5 Steps 10-12 |
| §6 documents | 10 |
| §7 non-goals | not implemented, on purpose |
| §8 R1-R3 | 2 and 6 (R1), 9 (R2), 4 (R3) |

### Spec ambiguities resolved in this plan

1. **`cd` before vitest (§4.2).** Read literally, it reds the real `paigasus-kernel-ts:test`, whose `cd` is inside a `( … )` subshell. A13 removes closed `( … )` groups before it looks for `cd` or `pushd`. A `cd` in the same shell, or in an open group that holds the vitest call, is still a row.
2. **`tsconfig: false` with no config (§4.3 item 5).** "Every config sets it" is true for an empty set. A13 skips the tsconfig files only when there is at least one config and all set it, because vitest with defaults loads them.
3. **The `--out-dir` match (D3).** The real value `.wasmpack-test-out` is relative to a `cd`. A13 accepts the value when it equals the alias target's directory or is a whole-segment suffix of it.
4. **tsconfig.json format.** The spec does not say how to read it. Two real files hold `//` comments. A13 removes comments with the same scanner, then uses `json.loads`. A failure is a row.
5. **An unresolvable value for a key that is not `@paigasus/` (§4.4 step 6).** The spec names a row only for `@paigasus/` keys, but also says that any form outside the grammar is a row. A13 fails closed: an unresolvable value is a row for any key. A bare package name for a non-`@paigasus/` key still adds nothing, as the spec says.
6. **The git environment and the error class (§4.5).** `task_inputs._git` did not clear `GIT_DIR` and `GIT_INDEX_FILE`, and its error class was not in `INFRA_ERRORS`. Task 4 changes `_git` (this also affects `repo:input-liveness`, safely) and adds the class.
7. **The count comment at `ci/actionlint/run.sh:2117`.** It counts `T_AFFECTED_SMOKE_REQUIRED_INPUTS`, which Task 7 grows. It already said 23 while the list held 29. Task 7 sets it to 34.
8. **`napi-glue-dts->kernel-test` (§4.12).** A13 never demands a `.d.ts`, so the plan predicts no change. Task 9 measures it.
9. **Task split.** A `check_` function that `collect_findings` does not call reds the SMA-542 guard. So the check and its registration are one task (5), and the tracked-file plumbing moved before it (4).
10. **Reachability (§4.6).** A13 also accepts a config that is an exact `inputFiles` entry of `repo:affected-smoke`. That entry schedules the gate as well as a glob.
11. **Walk rows printed once (§4.8).** A12a prints every `ts_workspace_packages` manifest row even when no `tsc` task exists. A13 drops each walk row whose text is in A12's walk rows.
12. **`projects` (§4.4 step 8).** A `projects` value that is not an array literal is also a row, because it can carry an alias that A13 does not read.

### Measurement steps

| Id | What | Where |
|---|---|---|
| M1 | Delete the `a13` tuple: the arity rows and the SMA-542 guard red | Task 5 Step 10 |
| M2 | `return []` in `check_ts_vitest_inputs`: every T2 row reds | Task 5 Step 11 |
| M3 | A13 on the real tree before any fix: 78 rows (65 + 13) | Task 5 Step 8 |
| M4 | The corpus pin reds when the parser stops seeing `tsconfig: false` | Task 6 Step 4 |
| M5 | The `run.sh` re-baseline output | Task 9 Steps 4 and 9 |

## Measurements

The planner measured M1, M2 and M3 on a scratch copy with the code of this plan; the expected outputs in Task 5 are those measurements. The implementer replaces each entry below with the output of the real branch.

- **M1:** measured on the branch at commit 8da009a5, 2026-10-10. The implementer did not edit the committed file. The mutant was an untracked copy in the same directory (`ci/affected-graph/_mutant_parity.py`), so `Path(__file__).parents[2]` is still the repository root. The copy had the whole 14-line `a13` tuple removed from `collect_findings`. After the run, the implementer deleted the copy. `git diff --stat` and `git status --short` were empty. The output is equal to the Task 5 Step 10 expectation. `rc=1`:

  ```text
    FAIL the real run never calls check_ts_vitest_inputs — a check that is defined but not invoked asserts nothing (SMA-542)
    FAIL collect_findings returned 14 entries, expected 15 — a check was added or dropped without updating EXPECTED_FINDING_KEYS
    FAIL collect_findings reported ('a1', 'a2', 'a3', 'a4-lint', 'a4-fmt', 'a5', 'a6', 'a7', 'a8', 'a9', 'a10', 'a11', 'a12a', 'a12b'), expected ('a1', 'a2', 'a3', 'a4-lint', 'a4-fmt', 'a5', 'a6', 'a7', 'a8', 'a9', 'a10', 'a11', 'a12a', 'a12b', 'a13') — a check was dropped, added or reordered in the findings list
  negative-control FAILED: the parity gate can pass vacuously
  ```

- **M2:** measured on the branch at commit 8da009a5, 2026-10-10, with the same sibling-copy method as M1. The copy had `return []` as the first statement of `check_ts_vitest_inputs`, after its docstring. `rc=1`. The `^  FAIL` count is `33`. `git diff --stat` and `git status --short` were empty after the copy was deleted. The first eleven lines are equal to the Task 5 Step 11 expectation:

  ```text
    FAIL A13 did not demand ts/pnpm-lock.yaml of c-ts:test
    FAIL A13 did not demand ts/packages/core/vitest.config.ts of c-ts:test
    FAIL A13 did not demand ts/packages/kernel/src/**/* of app-ts:test
    FAIL A13 did not demand ts/packages/proto/package.json of c-ts:test
    FAIL A13 did not demand ts/packages/core/testing/**/* of app-ts:test
    FAIL A13 did not demand rs/crates/bindings/wb/wb_bg.wasm of c-ts:test
    FAIL A13 did not demand rs/crates/bindings/wb/wb.js of c-ts:test
    FAIL A13 did not demand ts/packages/kernel/tsconfig.json of c-ts:test
    FAIL A13 did not demand ts/packages/kernel/tsconfig.json of k-ts:test
    FAIL A13 did not demand ts/tsconfig.base.json of k-ts:test
    FAIL A13 did not demand ts/packages/kernel/src/wasm.ts of c-ts:test
  ```

- **M3:** measured on the branch before commit 8da009a5, with the code of that commit, 2026-10-10. `python3 ci/affected-graph/cargo_moon_parity.py` (the real run) gave `rc=1`. The file holds 78 row lines (`^      `). The only title is the A13 title. The rows have three kinds: 64 `<target> inputs omit <path>` rows, 1 `deps omit contracts:generate` row (`paigasus-app-shell-ts:test`), and 13 `<config> matches no input of repo:affected-smoke` rows. 64 + 1 = 65 rows for Task 8, plus 13 rows for Task 7. This is equal to the Task 5 Step 8 expectation. The 13 configs are:

  ```text
  ts/apps/gateway-console/vitest.config.ts
  ts/apps/iam-console/vitest.config.ts
  ts/packages/paigasus-app-shell/vitest.config.ts
  ts/packages/paigasus-auth/vitest.config.ts
  ts/packages/paigasus-auth/vitest.containers.config.ts
  ts/packages/paigasus-console-core/vitest.config.ts
  ts/packages/paigasus-console-core/vitest.containers.config.ts
  ts/packages/paigasus-discovery/vitest.config.ts
  ts/packages/paigasus-discovery/vitest.containers.config.ts
  ts/packages/paigasus-kernel/vitest.config.ts
  ts/packages/paigasus-next-config/vitest.config.ts
  ts/packages/paigasus-sdk/vitest.config.ts
  ts/packages/paigasus-ui/vitest.config.ts
  ```

  The 65 Task 8 rows, per target: `gateway-console-ts:test` 2, `iam-console-ts:test` 2, `paigasus-app-shell-ts:test` 12 plus the `deps` row, `paigasus-auth-ts:test` 2, `paigasus-auth-ts:test-e2e` 2, `paigasus-console-core-ts:test` 9, `paigasus-console-core-ts:test-e2e` 14, `paigasus-discovery-ts:test` 4, `paigasus-discovery-ts:test-e2e` 5, `paigasus-kernel-ts:test` 2, `paigasus-next-config-ts:test` 2, `paigasus-proto-ts:test` 2, `paigasus-sdk-ts:test` 4, `paigasus-ui-ts:test` 2.
- **M4:** not yet run on the branch (Task 6 Step 4 writes it).
- **M5:** not yet run on the branch (Task 9 Step 9 writes it).
