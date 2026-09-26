<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-696 chart appVersion gate — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Set `charts/paigasus` `appVersion` to `0.1.0` and add `repo:helm-render` row
`8c chart-app-version`, which fails when the empty-tag fallback names an image that some chain never released.

**Architecture:** One pure function `check8c` in `ci/helm-render/helm_render.py`, with two
readers (`chart_app_version`, `release_tags`) that take injectable inputs. `run_checks()` reads
the inputs at its top, renders the chart once more with every tag cleared, and appends the row
after 8b. A negative-control fixture and a `ci_targets.py` pin prove the production call site.

**Tech Stack:** Python 3 (stdlib + PyYAML) in `ci/helm-render/`, bash 3.2-compatible `run.sh`,
helm through proto, git.

**Spec:** `docs/superpowers/specs/2026-09-26-sma-696-chart-app-version-design.md` (approved
2026-09-26). Read it before any task.

## Global Constraints

- Work only in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-696`,
  branch `feature/sma-696-chart-app-version`. Check `git branch --show-current` before each commit.
- Prefix every shell with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Run `ci/helm-render/run.sh` with `/bin/bash` (3.2). Do not use a Docker container: git cannot
  read the worktree's tags there.
- Every new source file starts with an SPDX header (`# SPDX-License-Identifier: Apache-2.0`).
- Conventional commits, scope `ci` for gate code, `repo` for the chart and docs. End each message
  with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Put no `#NNN` or
  `token: value` line in a commit body (commitlint `footer-leading-blank`).
- Never `git commit --amend`, never `git reset`, never `--no-verify`, never `git stash` without a
  unique message. Do not install host software (`brew install` and similar are forbidden).
- Run commands in the foreground. Do not start a background job and then wait for it.
- All prose (comments, docs, messages) is ASD-STE100 Simplified Technical English: short
  sentences, active voice, no idiom.
- The row label is exactly `8c chart-app-version`. The fixture directory is exactly
  `app-version-unreleased`. Its `appVersion` is exactly `"0.1.0-helm-render-unreleased"`.
- `UNRELEASED_CHAINS` ships EMPTY (`{}`).

## Review Focus

1. **A user's git config changes the tag output.** `column.ui=always` must not merge tags onto one
   line. `release_tags` uses `git for-each-ref`, and its self-test stub prints one tag per line
   plus a blank line. Task 1 pins this.
2. **A version that is a prefix of a tag** (`0.1` against `paigasus-iam-v0.1.0`) or a key that is
   a prefix of another key (`iam` against `iam-console`) must fail. Task 1 pins both.
3. **An unquoted numeric `appVersion`** (`appVersion: 1.0`) must fail with "quote", not crash and
   not rc 2. Task 1 pins the pure-function case; Task 1's `chart_app_version` float row pins the
   reader.
4. **The fixture chart, not the repository chart, supplies `appVersion`.** Task 2's negative
   control and Task 3's mutation 3 pin it.
5. **A clone with no tags** must be rc 2 with a "fetch the tags" message, not a row failure that
   reads like a chart defect. Task 1 pins it.

---

### Task 1: Row 8c as pure functions, with self-tests

**Files:**
- Modify: `ci/helm-render/helm_render.py` (constants near `CLEARED_TAGS` ~line 94; new functions
  after `check8b` ~line 815; new self-test rows after the check 8 rows ~line 1241)

**Interfaces:**
- Produces:
  - `UNRELEASED_CHAINS: dict[str, str]` — chain key to reason. Value `{}`.
  - `APP_VERSION_ROW = "8c chart-app-version"`
  - `chart_app_version(chart) -> object` — the raw `appVersion` value, or `None`.
  - `release_tags(run=subprocess.run) -> frozenset[str]`
  - `check8c(app_version, registry, tags, fallback_docs, unreleased=UNRELEASED_CHAINS) -> Row`

- [ ] **Step 1: Write the failing self-test rows**

Insert this block in `self_test()` AFTER the `with tempfile.TemporaryDirectory(prefix="helm-render-8-")`
block ends (its last line is
`expect_infra("chain_registry: no [chain] table", lambda: chain_registry(root / "chains.toml"))`),
at the indent of `self_test`'s body, before the comment `# ---- the exit-code contract`:

```python
    # ---- row 8c (SMA-696): the empty-tag fallback names an image every chain released
    r8c = (APP_VERSION_ROW,)
    tags8c = frozenset(f"paigasus-{k}-v0.1.0" for k in reg)
    fallback = synthetic(both, tags={"iam": "0.1.0", "gateway": "0.1.0"}, backend_tag="0.1.0")

    def expect_detail(label, row, word):
        # A failure row must fail for ITS reason: the detail must hold `word`.
        if row.ok or word not in row.detail:
            failures.append(f"{label}: expected a FAIL whose detail holds {word!r}, got {row}")

    expect("check8c good", [check8c("0.1.0", reg, tags8c, fallback)], passing=r8c)
    expect_detail("check8c one chain has no release",
                  check8c("0.1.0", reg, tags8c - {"paigasus-gateway-console-v0.1.0"}, fallback),
                  "paigasus-gateway-console-v0.1.0")
    fallback_020 = synthetic(both, tags={"iam": "0.2.0", "gateway": "0.2.0"}, backend_tag="0.2.0")
    expect_detail("check8c prefix trap: iam-console's tag does not release iam",
                  check8c("0.2.0", reg, frozenset({"paigasus-iam-console-v0.2.0", "paigasus-gateway-console-v0.2.0"}), fallback_020),
                  "paigasus-iam-v0.2.0")
    fallback_01 = synthetic(both, tags={"iam": "0.1", "gateway": "0.1"}, backend_tag="0.1")
    expect_detail("check8c prefix trap: 0.1 is not 0.1.0", check8c("0.1", reg, tags8c, fallback_01), "paigasus-iam-v0.1")
    # synthetic() defaults every image to :0.0.0, so only the missing tags can red this row.
    expect_detail("check8c the SMA-696 value 0.0.0", check8c("0.0.0", reg, tags8c, synthetic(both)), "paigasus-iam-v0.0.0")
    expect_detail("check8c empty", check8c("", reg, tags8c, fallback), "no tag")
    expect_detail("check8c not a string", check8c(1.0, reg, tags8c, fallback), "quote")
    expect_detail("check8c None", check8c(None, reg, tags8c, fallback), "quote")
    v_render = copy.deepcopy(fallback)
    _containers(_find(v_render, "Deployment", "r-iam-console"))[0]["image"] = "repo/iam-console:v0.1.0"
    expect_detail("check8c the rendered fallback differs", check8c("0.1.0", reg, tags8c, v_render), "r-iam-console")
    no_gc = tags8c - {"paigasus-gateway-console-v0.1.0"}
    expect("check8c unreleased key with no tags",
           [check8c("0.1.0", reg, no_gc, fallback, unreleased={"gateway-console": "new chain"})], passing=r8c)
    expect_detail("check8c unreleased key that released",
                  check8c("0.1.0", reg, tags8c, fallback, unreleased={"gateway-console": "new chain"}),
                  "UNRELEASED_CHAINS")
    expect_detail("check8c unreleased key that is not a chain",
                  check8c("0.1.0", reg, tags8c, fallback, unreleased={"billing": "x"}), "billing")
    if UNRELEASED_CHAINS != {}:
        failures.append(f"UNRELEASED_CHAINS must ship empty, got {UNRELEASED_CHAINS}")

    class _GitProc:
        def __init__(self, rc, out=""):
            self.returncode, self.stdout, self.stderr = rc, out, "stub stderr"

    def git_missing(cmd, **_kw):
        raise FileNotFoundError("git")

    expect_infra("release_tags: no tags", lambda: release_tags(run=lambda cmd, **_kw: _GitProc(0, "")))
    expect_infra("release_tags: git exits 128", lambda: release_tags(run=lambda cmd, **_kw: _GitProc(128)))
    expect_infra("release_tags: git is not found", lambda: release_tags(run=git_missing))
    got_tags = release_tags(run=lambda cmd, **_kw: _GitProc(0, "paigasus-iam-v0.1.0\n\npaigasus-gateway-v0.1.0\n"))
    if got_tags != frozenset({"paigasus-iam-v0.1.0", "paigasus-gateway-v0.1.0"}):
        failures.append(f"release_tags: expected exactly the two tags, got {sorted(got_tags)}")
    seen_cmd = []
    release_tags(run=lambda cmd, **_kw: seen_cmd.append(cmd) or _GitProc(0, "paigasus-iam-v0.1.0\n"))
    if not seen_cmd or "for-each-ref" not in seen_cmd[0]:
        failures.append(f"release_tags: must use git for-each-ref (plumbing), got {seen_cmd}")
    with tempfile.TemporaryDirectory(prefix="helm-render-8c-") as tmp:
        chart_dir = Path(tmp)
        (chart_dir / "Chart.yaml").write_text('apiVersion: v2\nname: x\nversion: 0.1.0\nappVersion: "0.4.0"\n')
        if chart_app_version(chart_dir) != "0.4.0":
            failures.append("chart_app_version: did not return 0.4.0")
        (chart_dir / "Chart.yaml").write_text("apiVersion: v2\nname: x\nversion: 0.1.0\n")
        if chart_app_version(chart_dir) is not None:
            failures.append("chart_app_version: a Chart.yaml with no appVersion must give None")
        (chart_dir / "Chart.yaml").write_text("apiVersion: v2\nname: x\nversion: 0.1.0\nappVersion: 1.0\n")
        if chart_app_version(chart_dir) != 1.0:
            failures.append("chart_app_version: an unquoted 1.0 must come back as the float, for the row to reject")
        (chart_dir / "Chart.yaml").write_text("a: [\n")
        expect_infra("chart_app_version: bad YAML", lambda: chart_app_version(chart_dir))
        (chart_dir / "Chart.yaml").unlink()
        expect_infra("chart_app_version: no Chart.yaml", lambda: chart_app_version(chart_dir))
```

Note: `reg` is the three-chain registry that the check 8 rows define (`iam`, `iam-console`,
`gateway-console`). `synthetic(both, ...)` renders the iam backend as `repo/iam:<backend_tag>`
and each console as `repo/<zone>-console:<tag>`.

- [ ] **Step 2: Run the self-test and see it fail**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; /bin/bash ci/helm-render/run.sh --self-test; echo rc=$?`
Expected: a non-zero rc, with a `NameError` for `APP_VERSION_ROW` (or `check8c`) in the output.

- [ ] **Step 3: Add the constants**

After the `CLEARED_TAGS = (...)` block, add:

```python
# Row 8c (SMA-696 D2a). A chain key that has no release yet, mapped to the reason. Row 8c needs no
# tag for a listed key, and FAILS when a listed key has a tag, so an entry goes away at the first
# release. It ships EMPTY; the self-test asserts that.
UNRELEASED_CHAINS: dict[str, str] = {}
APP_VERSION_ROW = "8c chart-app-version"
```

- [ ] **Step 4: Add the readers and `check8c`**

Directly after `check8b`'s `return _row("8b default-image-render", body)`, add:

```python
def chart_app_version(chart):
    """The raw appVersion of `<chart>/Chart.yaml`, or None. A missing or non-string value is NOT
    an infrastructure error: row 8c fails on it. An unreadable file or bad YAML is rc 2."""
    path = Path(chart) / "Chart.yaml"
    try:
        doc = yaml.safe_load(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, yaml.YAMLError) as exc:
        raise InfraError(f"cannot read {path}: {exc}") from exc
    return doc.get("appVersion") if isinstance(doc, dict) else None


def release_tags(run=subprocess.run):
    """Every `paigasus-*` git tag of the repository, one per line of plumbing output. `git tag
    --list` is porcelain and follows column.ui, so it can put several tags on one line. No tag at
    all is rc 2: a checkout with no tags, not a chart defect. `run` is a parameter only so
    self_test() can drive this with no git."""
    cmd = ["git", "-C", str(REPO_ROOT), "for-each-ref", "--format=%(refname:lstrip=2)", "refs/tags/paigasus-*"]
    try:
        proc = run(cmd, capture_output=True, text=True, check=False)
    except FileNotFoundError as exc:
        raise InfraError(f"git is not on PATH: {exc}") from exc
    if proc.returncode != 0:
        raise InfraError(f"git for-each-ref exited {proc.returncode}: {proc.stderr.strip()}")
    tags = frozenset(line.strip() for line in proc.stdout.splitlines() if line.strip())
    if not tags:
        raise InfraError("git lists no paigasus-* tag. Fetch the tags (fetch-depth: 0, or git fetch --tags)")
    return tags


def check8c(app_version, registry, tags, fallback_docs, unreleased=UNRELEASED_CHAINS):
    """Row 8c (SMA-696): the chart appVersion is a version that EVERY chain released, so an empty
    image.tag falls back to a real image. `fallback_docs` is the iam+gateway render with every
    image.tag cleared; each of its Deployment images must carry exactly that tag."""

    def body():
        if not isinstance(app_version, str):
            return [f"Chart.yaml appVersion is {app_version!r}, not a string. Quote appVersion in Chart.yaml"]
        if not app_version:
            return ["Chart.yaml appVersion is empty, so an empty image.tag falls back to no tag"]
        problems = []
        for key in sorted(unreleased):
            if key not in registry:
                problems.append(f"UNRELEASED_CHAINS lists {key!r}, which is no chain in ci/images/chains.toml")
            elif any(t.startswith(f"paigasus-{key}-v") for t in tags):
                problems.append(f"UNRELEASED_CHAINS lists {key!r}, but that chain has a release tag. Remove the entry")
        missing = [f"paigasus-{key}-v{app_version}" for key in sorted(registry)
                   if key not in unreleased and f"paigasus-{key}-v{app_version}" not in tags]
        if missing:
            problems.append(f"no release tag {missing}. Move appVersion only after every chain released it; "
                            "run git fetch --tags if the tags are not local")
        for dep in _of_kind(fallback_docs, "Deployment"):
            pod = _get(dep, "spec", "template", "spec")
            for container in (pod.get("containers") or []) + (pod.get("initContainers") or []):
                image = str(container.get("image"))
                tag = image.rsplit(":", 1)[1] if ":" in image else ""
                if tag != app_version:
                    problems.append(f"{_name(dep)}/{container.get('name')}: the empty-tag render names {image!r}, not the tag {app_version!r}")
        return problems

    return _row(APP_VERSION_ROW, body)
```

Check: `_of_kind`, `_get`, `_name`, `_row` already exist in this file. Do not add new imports;
`yaml`, `subprocess`, `Path` are already imported.

- [ ] **Step 5: Run the self-test and see it pass**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; /bin/bash ci/helm-render/run.sh --self-test; echo rc=$?`
Expected: `rc=0`. The production run is not wired yet, so do not run the full gate here.

- [ ] **Step 6: Commit**

```bash
git add ci/helm-render/helm_render.py
git commit -m "feat(ci): add helm-render row 8c as pure functions (SMA-696)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Wire row 8c, set appVersion 0.1.0, add the fixture and the pin

**Files:**
- Modify: `ci/helm-render/helm_render.py` (`EXPECTED_ROW_LABELS`, `run_checks`, the arity check
  `!= 23`)
- Modify: `charts/paigasus/Chart.yaml:7`
- Create: `ci/helm-render/fixtures/app-version-unreleased/Chart.yaml`
- Modify: `ci/helm-render/run.sh:36-43` (`FIXTURE_TABLE`)
- Modify: `ci/affected-graph/ci_targets.py:1380` and `:1404-1409` (`HELM_RENDER_SH_CALL_SITES`)

**Interfaces:**
- Consumes: `APP_VERSION_ROW`, `chart_app_version`, `release_tags`, `check8c` from Task 1;
  `CLEARED_TAGS`, `chain_registry`, `helm_template`, `parse_docs` (existing).

- [ ] **Step 1: Wire the row (red first)**

In `EXPECTED_ROW_LABELS`, add `"8c chart-app-version",` after `"8b default-image-render",`.
In the self-test, change both `23` in
`if len(EXPECTED_ROW_LABELS) != 23:` and its message to `24`.

In `run_checks`, add the two reads at the TOP, directly after the `except OSError ... raise
InfraError(...)` block of the source reads:

```python
    # Row 8c (SMA-696): read the inputs first, so a git fault fails fast, before the renders.
    app_version = chart_app_version(chart)
    tags = release_tags()
```

Then, directly after `rows.append(check8b(values, both_docs))`, add:

```python
    fallback_docs = parse_docs(helm_template(chart, ("gateway", "iam"), CLEARED_TAGS))
    rows.append(check8c(app_version, registry, tags, fallback_docs))
```

- [ ] **Step 2: Run the full gate and see row 8c fail on `0.0.0`**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; /bin/bash ci/helm-render/run.sh; echo rc=$?`
Expected: `rc=1`, a line starting `FAIL  [8c chart-app-version]` that names the missing tags
`paigasus-gateway-console-v0.0.0` … `paigasus-iam-v0.0.0`, and every other row PASS.

- [ ] **Step 3: Set appVersion**

`charts/paigasus/Chart.yaml` line 7: `appVersion: "0.0.0"` becomes `appVersion: "0.1.0"`.

- [ ] **Step 4: Run the full gate and see it pass**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; /bin/bash ci/helm-render/run.sh; echo rc=$?`
Expected: `rc=0`, a `PASS  [8c chart-app-version]` line, and `all 24 rows passed`. If a golden
file differs, stop and report: `appVersion` must not render into the goldens (measured: it does
not; only the empty-tag fallback reads it).

- [ ] **Step 5: Add the fixture**

```bash
mkdir -p ci/helm-render/fixtures/app-version-unreleased
sed 's/^appVersion: "0.1.0"$/appVersion: "0.1.0-helm-render-unreleased"/' charts/paigasus/Chart.yaml \
  > ci/helm-render/fixtures/app-version-unreleased/Chart.yaml
diff charts/paigasus/Chart.yaml ci/helm-render/fixtures/app-version-unreleased/Chart.yaml
```

Expected diff: exactly one changed line, the `appVersion` line. The SPDX header is kept.

- [ ] **Step 6: Add the FIXTURE_TABLE row**

In `ci/helm-render/run.sh`, add this line after the `'security-context|…'` line, inside the
array:

```bash
  'app-version-unreleased|8c chart-app-version|'
```

- [ ] **Step 7: Run the negative control and see the new fixture OK**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; /bin/bash ci/helm-render/run.sh --negative-control; echo rc=$?`
Expected: `rc=0`, and a line `negative-control OK [app-version-unreleased]: rc 3, 1 row(s) failed, the named row(s) among them`.
If the count is not 1, record the real count and the rows. With explicit tags, no other row reads
`appVersion` (row 3b bumps its own copy), so expect 1.

- [ ] **Step 8: Pin the fixture row**

In `ci/affected-graph/ci_targets.py`, in `HELM_RENDER_SH_CALL_SITES`, add after
`"'security-context|4 security-context iam;4 security-context iam+gateway|'",`:

```python
    "'app-version-unreleased|8c chart-app-version|'",
```

At line ~1380 change `the six FIXTURE_TABLE rows` to `the seven FIXTURE_TABLE rows`.

- [ ] **Step 9: Prove the pin bites, then passes**

Run: `python3 ci/affected-graph/ci_targets.py; echo rc=$?`
Expected: `rc=0`.
Then remove the new line from `run.sh` with the Edit tool (not `git checkout`), and run again.
Expected: non-zero rc and a message that names `ci/helm-render/run.sh: 'app-version-unreleased|8c chart-app-version|'`.
Put the line back with the Edit tool and run again. Expected: `rc=0`.

- [ ] **Step 10: Run all three helm-render modes**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; for m in --self-test --negative-control ""; do /bin/bash ci/helm-render/run.sh $m; echo "mode=$m rc=$?"; done`
Expected: `rc=0` for all three.

- [ ] **Step 11: Commit**

```bash
git add ci/helm-render/helm_render.py charts/paigasus/Chart.yaml ci/helm-render/fixtures/app-version-unreleased/Chart.yaml ci/helm-render/run.sh ci/affected-graph/ci_targets.py
git commit -m "feat(ci): gate the chart appVersion with helm-render row 8c (SMA-696)

Chart.yaml appVersion moves from 0.0.0 to 0.1.0, a version every image chain released.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The deletion proof and the helm-render README

**Files:**
- Modify (temporarily, then restore): `ci/helm-render/helm_render.py`
- Modify: `ci/helm-render/README.md` (checks table ~line 41-42; negative-control table ~line 71-78;
  Delete-the-feature record ~line 95-110; Tool resolution ~line 112-120; Running it locally
  ~line 144-155)

Make each mutation with the Edit tool and undo it with the Edit tool. Do NOT use
`git checkout --` or `git restore`: they are safe here only because Task 2 is committed, but the
habit discards uncommitted fixes. Check `git diff --stat` is empty after each undo.

- [ ] **Step 1: Mutation 1 — remove the call**

Delete the line `rows.append(check8c(app_version, registry, tags, fallback_docs))` in
`run_checks`. Run:
`export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; /bin/bash ci/helm-render/run.sh; echo rc=$?`
Expected: `rc=2` and `helm-render row inventory does not match EXPECTED_ROW_LABELS: missing ['8c chart-app-version']`.
Record the exact line. Undo.

- [ ] **Step 2: Mutation 2 — the body always passes**

In `check8c`, change `return _row(APP_VERSION_ROW, body)` to `return _row(APP_VERSION_ROW, lambda: [])`.
Run `--self-test` and `--negative-control` (as above). Expected: `--self-test` non-zero with the
check8c rows red; `--negative-control` rc 1 with `negative-control FAILED: the module passed a mutated chart (rc=0)`
for `app-version-unreleased`. Record both. Undo.

- [ ] **Step 3: Mutation 3 — read the wrong chart**

In `run_checks`, change `app_version = chart_app_version(chart)` to
`app_version = chart_app_version(REPO_ROOT / "charts" / "paigasus")`. Run `--negative-control`.
Expected: the control still reports OK. The fallback render still uses the fixture chart, so its
images carry `:0.1.0-helm-render-unreleased`, and § 3.2 item 4 fails the row. This is a
correction to spec § 4.3 item 3, which expected FAILED: item 4 is a second guard for the same
mistake. Record it as "caught by § 3.2 item 4". Then apply mutation 4 (Step 4) on top of this
one and run `--negative-control` again. Expected: rc 1, `FAILED` for `app-version-unreleased`,
because item 2 alone then reads the repository chart's released `0.1.0`. Record it. Undo both.

- [ ] **Step 4: Mutation 4 — remove the render assertion**

In `check8c`, replace the `for dep in _of_kind(fallback_docs, "Deployment"):` loop body's
`problems.append(...)` line with `pass`. Run `--self-test`. Expected: non-zero, and the row
`check8c the rendered fallback differs` is red. Record. Undo.

- [ ] **Step 5: Confirm a clean tree**

Run: `git diff --stat; for m in --self-test --negative-control ""; do /bin/bash ci/helm-render/run.sh $m; echo "mode=$m rc=$?"; done`
Expected: empty diff, three `rc=0`.

- [ ] **Step 6: Update the README**

In `ci/helm-render/README.md`:

1. Checks table, after the `8b default-image-render` row, add:
   `| \`8c chart-app-version\` | The empty-tag fallback names a released image (SMA-696) | \`Chart.yaml\` \`appVersion\` is not a non-empty string; or a chain of \`ci/images/chains.toml\` that is not in \`UNRELEASED_CHAINS\` has no git tag \`paigasus-<key>-v<appVersion>\`; or an \`UNRELEASED_CHAINS\` key has a release tag or is no chain; or a Deployment image in the render with every \`image.tag\` cleared does not carry the tag \`appVersion\` |`
2. Negative-control table, add the row:
   `| \`app-version-unreleased\` | \`appVersion\` is \`0.1.0-helm-render-unreleased\`, which the pipeline never tags | \`8c chart-app-version\` | — | <the rows measured in Task 2 Step 7> |`
   and change the sentence "Measured one fixture at a time on 2026-09-22" to add "; `app-version-unreleased` on 2026-09-26".
3. In the Delete-the-feature record, change `the 35 \`HELM_RENDER_SH_CALL_SITES\` lines` to `the 36`, and add a paragraph "Row 8c (SMA-696), measured on <date>:" with the four results from Steps 1-4, in the style of the Row 7 paragraph.
4. In Tool resolution, add a bullet: "`git` is not pinned. Row 8c runs `git for-each-ref` for the `paigasus-*` tags. The tags are an input that Moon does not hash, so a cached PASS survives a deleted tag. A checkout with no `paigasus-*` tag is rc 2. CI checks out with `fetch-depth: 0`."
5. In Running it locally, add: "Row 8c reads the LOCAL tags. A local-only tag gives a green that CI does not give, and a tag that is not fetched gives a red. Run `git fetch --tags` first. CI is the authority. Run the gate on the host under `/bin/bash`, not in the Docker workaround: a worktree's `.git` links to a directory on the host, so git fails in the container and the module exits rc 2."

- [ ] **Step 7: Commit**

```bash
git add ci/helm-render/README.md
git commit -m "docs(ci): record helm-render row 8c and its deletion proof (SMA-696)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: The other docs and comments

**Files:**
- Modify: `charts/paigasus/values.yaml` (four tag comments, ~lines 17-19, 29-30, 52-53, 66-68)
- Modify: `charts/paigasus/README.md` (§ "The default image tags", ~lines 150-158)
- Modify: `docs/ops/RUNBOOK-chart.md:14` (and the IAM image row below it if it repeats the fallback)
- Modify: `charts/CLAUDE.md` (the whole-file fixture bullet ~line 22; the last bullet ~lines 33-36)
- Modify: `.github/CLAUDE.md` (after the bullet "A person sets a service or console version by hand", ~line 289)
- Modify: `ci/images/chains.toml` (header lines 11 and 19-21)
- Modify: `moon.yml` (the `helm-render` WHY comment, ~lines 986-989)
- Modify: `ts/packages/paigasus-auth/src/core/session.ts:24-26`

- [ ] **Step 1: values.yaml**

- iam console comment: replace "An empty\n        # tag falls back to .Chart.AppVersion, which names no published image." with
  "An empty\n        # tag falls back to .Chart.AppVersion, a version every image released (row 8c)."
  Keep the comment lines under 100 characters and the indent unchanged.
- gateway backend comment: replace "the pin stops a later chart that\n        # deploys it from falling back to 0.0.0." with
  "the pin stops a later chart that\n        # deploys it from falling back to an older appVersion."
- The iam backend and gateway console comments name no fallback; leave them.

- [ ] **Step 2: charts/paigasus/README.md**

Replace the last two sentences of § "The default image tags" ("The tags no longer default to
`appVersion`. An empty tag still falls back to `.Chart.AppVersion`, which is `0.0.0` and names no
published image.") with:

```markdown
The tags no longer default to `appVersion`. An empty tag falls back to `.Chart.AppVersion`.
`repo:helm-render` row 8c requires `appVersion` to be a version that every image released: the git
tag `paigasus-<key>-v<appVersion>` must exist for each chain. So `appVersion` is the fallback tag,
not the deployed version. It can be older than the pinned tags, and `helm list` then shows that
older value. To move `appVersion`, use a later pull request, after every image released the new
version. A pull request that bumps the versions and `appVersion` together fails row 8c until the
release.
```

- [ ] **Step 3: RUNBOOK**

In `docs/ops/RUNBOOK-chart.md` line 14, replace "An empty `tag` falls back to the chart `appVersion` (`0.0.0`), which names no published image" with
"An empty `tag` falls back to the chart `appVersion`. That is a released version of every image, but it can be older than the pinned tags. It is the fallback tag, not the deployed version".
Grep the file for any other `0.0.0` fallback text and fix it the same way.

- [ ] **Step 4: charts/CLAUDE.md**

- Whole-file fixture bullet: "Four negative-control fixtures are whole-file copies of `templates/_helpers.tpl` (two) and `templates/console-deployment.yaml` (two)" becomes
  "Five negative-control fixtures are whole-file copies: of `templates/_helpers.tpl` (two), `templates/console-deployment.yaml` (two) and `Chart.yaml` (one, `app-version-unreleased`)". Adjust "An edit to either live file" to "An edit to any of these live files".
- Last bullet: replace "An empty tag falls back to\n  `appVersion` (`0.0.0`), which names no published image." with
  "An empty tag falls back to\n  `appVersion` (`0.1.0`). Row 8c requires a git tag `paigasus-<key>-v<appVersion>` for every chain\n  (SMA-696). Move `appVersion` in a later PR, after every chain released that version."

- [ ] **Step 5: .github/CLAUDE.md**

After the paragraph that ends "Row 8 of `repo:helm-render` fails when the tag differs.", add a
new paragraph:

```markdown
  The chart `appVersion` does NOT move in that pull request. `repo:helm-render` row 8c requires the
  git tag `paigasus-<key>-v<appVersion>` for every chain, and a tag exists only after the release
  (SMA-696). Move `appVersion` in a later pull request, after every chain released the version.
```

- [ ] **Step 6: chains.toml header**

- Line `#   ci/helm-render/helm_render.py     kind, version_file, ghcr (row 8a)` becomes
  `#   ci/helm-render/helm_render.py     the keys (row 8c), kind, version_file, ghcr (row 8a)`.
- After the sentence "It also needs a CHAIN_APPROVALS entry in release_guard.py. V16 checks both the release.yml jobs and that entry." add:
  `# Until the new chain's first release, it also needs an UNRELEASED_CHAINS entry in`
  `# ci/helm-render/helm_render.py; row 8c fails when that entry stays after the release.`

- [ ] **Step 7: moon.yml**

In the `helm-render` WHY comment, after "ci/images/chains.toml and the four version files are what
row 8a reads (SMA-688).", add:

```yaml
    # Row 8c (SMA-696) reads Chart.yaml and the `paigasus-*` git tags. The tags are no file
    # input, so a cached PASS survives a deleted tag. Chart.yaml is an input (charts/**/*), so an
    # appVersion change always runs the row again.
```

- [ ] **Step 8: session.ts**

Replace "Both zones default\n   * their image tag to `.Chart.AppVersion` (charts/paigasus/values.yaml), so the mixed state lasts\n   * only for the rollout." with
"Both zones pin\n   * their image tag in charts/paigasus/values.yaml, so the mixed state lasts\n   * only for the rollout."
Keep the ` * ` prefix and the line width of the block.

- [ ] **Step 9: Sweep for stale text**

Run:
`git grep -nE "appVersion\W+\(?\`?0\.0\.0|AppVersion.{0,40}0\.0\.0|default (their|the) image tag|six FIXTURE_TABLE|35 \`HELM_RENDER|expected 23|all 23 rows" -- ':!docs/superpowers'`
Expected: no hit, except `helm_render.py`'s `0.0.0-helm-render-bump` (row 3b) and row 8a's own
`0.0.0` rule. Fix any other hit in the same way.

- [ ] **Step 10: Check the formatters and the gates that read these files**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec prettier --check packages/paigasus-auth/src/core/session.ts
for m in --self-test --negative-control ""; do /bin/bash ci/helm-render/run.sh $m; echo "mode=$m rc=$?"; done
python3 ci/affected-graph/ci_targets.py; echo rc=$?
```
Expected: prettier reports the file as formatted; three helm-render `rc=0`; `ci_targets.py` `rc=0`.

- [ ] **Step 11: Commit**

```bash
git add charts/paigasus/values.yaml charts/paigasus/README.md docs/ops/RUNBOOK-chart.md charts/CLAUDE.md .github/CLAUDE.md ci/images/chains.toml moon.yml ts/packages/paigasus-auth/src/core/session.ts
git commit -m "docs(repo): describe the appVersion fallback and row 8c (SMA-696)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Local gate graph

- [ ] **Step 1: Run the affected gates that this branch selects**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git fetch -q origin main
/bin/bash ci/affected-graph/run.sh --negative-control; echo "affected-smoke nc rc=$?"
/bin/bash ci/affected-graph/run.sh; echo "affected-smoke rc=$?"
```
Expected: both `rc=0`. `repo:affected-smoke` needs system bash 3.2.

- [ ] **Step 2: Run the TypeScript checks for the comment edit**

Run: `moon run paigasus-auth-ts:test 2>&1 | tail -5` and `moon ci :lint :typecheck :fmt --base origin/main 2>&1 | tail -15`.
Expected: both pass. A comment-only edit must change no test result.

- [ ] **Step 3: Report**

Report every rc. If a gate fails for a host reason (the 512-byte pipe, a bash version), name the
reason and the memory note that describes it. Do not call it green.
