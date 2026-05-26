# Moon Configuration (SMA-356) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire Moon as the polyglot task orchestrator for `paigasus-core` — workspace project graph, pinned toolchains, global task defaults, per-language generator templates, Moon-owned CODEOWNERS — such that `moon ci :build` runs clean on the (still empty) workspace.

**Architecture:** Pure configuration. The project globs match zero directories today (workspace dirs hold only READMEs), so Moon resolves **zero projects** and the verification gates are trivially satisfiable. Moon itself is installed and version-pinned via `proto`/`.prototools`; language toolchains are *declared* in `.moon/toolchain.yml` but not provisioned (no tasks to run). Generator templates are real `moon generate` scaffolds, one per language, parameterized by an `archetype` enum.

**Tech Stack:** Moon (moonrepo.dev), proto (toolchain manager), Git/GitHub. Config is YAML + Tera (template engine). No application code.

**Spec:** [`docs/superpowers/specs/2026-05-26-moon-configuration-design.md`](../specs/2026-05-26-moon-configuration-design.md)

---

## Preconditions

- Current branch is `feature/sma-356-set-up-moon-configuration` (already created; the spec is already committed on it).
- `node` and `curl` are available on the machine (used to resolve current-stable versions). Verify:

```bash
git branch --show-current   # → feature/sma-356-set-up-moon-configuration
node --version && curl --version | head -1
```

- `.moon/.gitkeep` exists (placeholder from SMA-355) and will be deleted in Task 2.

## File structure (what this plan creates)

| File | Responsibility |
|------|----------------|
| `.prototools` | Pin the Moon binary version (proto-managed) for reproducible `moon` runs. |
| `.moon/workspace.yml` | Project globs, VCS + GitHub provider, Moon-owned CODEOWNERS (`sync`), generator registration. |
| `.moon/toolchain.yml` | Pin Rust / Node+pnpm / Python+uv at current-stable, resolved at build time. |
| `.moon/tasks.yml` | Global file groups (`sources`/`tests`), `implicitInputs`, default task options. |
| `.moon/templates/rust/{template.yml,moon.yml}` | Generator: Rust `library`\|`service`. |
| `.moon/templates/python/{template.yml,moon.yml}` | Generator: Python `library`\|`service`. |
| `.moon/templates/typescript/{template.yml,moon.yml}` | Generator: TS `library`\|`app`. |
| `.github/CODEOWNERS` (generated) | Moon-synced owners; replaces the static root `CODEOWNERS`. |
| `CONTRIBUTING.md` (edit) | New "Local development setup" subsection: `proto` → `moon` install order. |
| _delete_ `.moon/.gitkeep`, `CODEOWNERS` (root) | Remove interim placeholder and the now-redundant static owners file. |

**Commit scopes:** use Conventional Commits with an allowlisted scope (`repo`, `docs`, `ci`) to stay compatible with the commitlint config landing in SMA-371. End every commit body with the `Co-Authored-By` trailer.

---

## Task 1: Install + pin Moon via proto

**Files:**
- Create: `.prototools`

- [ ] **Step 1: Install proto and put it on PATH**

Run:
```bash
bash <(curl -fsSL https://moonrepo.dev/install/proto.sh) --yes
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
proto --version
```
Expected: prints a proto version (e.g. `proto 0.4x.x`). If `proto` is not found afterward, the install script printed shell-setup instructions — apply the `export PATH=...` line above for this session.

- [ ] **Step 2: Resolve the latest stable Moon version and pin it in `.prototools`**

Run (uses the installed `node` to parse JSON — no `jq` dependency):
```bash
MOON=$(curl -fsSL https://api.github.com/repos/moonrepo/moon/releases/latest \
  | node -e 'const d=JSON.parse(require("fs").readFileSync(0));console.log(d.tag_name.replace(/^v/,""))')
echo "Resolved moon=$MOON"
printf 'moon = "%s"\n' "$MOON" > .prototools
cat .prototools
```
Expected: `.prototools` contains a single line like `moon = "1.39.2"` (exact patch will be today's latest).

- [ ] **Step 3: Install the pinned Moon and verify**

Run:
```bash
proto install
moon --version
```
Expected: `moon --version` prints the same version pinned in `.prototools`.

- [ ] **Step 4: Commit**

```bash
git add .prototools
git commit -m "$(cat <<'EOF'
chore(repo): pin Moon binary via .prototools

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `.moon/workspace.yml` + remove `.gitkeep`

**Files:**
- Create: `.moon/workspace.yml`
- Delete: `.moon/.gitkeep`

- [ ] **Step 1: Write `.moon/workspace.yml`**

Create the file with exactly this content:
```yaml
$schema: 'https://moonrepo.dev/schemas/workspace.json'

projects:
  - 'contracts'
  - 'rs/crates/libs/*'
  - 'rs/crates/bindings/*'
  - 'rs/crates/services/*'
  - 'py/packages/*'
  - 'ts/packages/*'
  - 'ts/apps/*'

vcs:
  client: 'git'       # Moon 2.x renamed `manager` → `client`.
  defaultBranch: 'main'
  provider: 'github'
  # `hooks` intentionally left unset — lefthook owns .git/hooks (SMA-371).

codeowners:
  # Field is `sync`, NOT `syncOnRun` (the AC's original wording was invalid for
  # current Moon — verified against moonrepo.dev config/workspace docs).
  sync: true
  globalPaths:
    '*': ['@SMK1085']

generator:
  templates:
    - './.moon/templates'
```

- [ ] **Step 2: Delete the interim placeholder**

Run:
```bash
git rm .moon/.gitkeep
```
Expected: `.moon/` now holds `workspace.yml` (a real tracked file), so the directory stays tracked without the placeholder.

- [ ] **Step 3: Verify Moon parses the workspace and resolves zero projects**

Run:
```bash
moon query projects
```
Expected: completes successfully and lists **no** projects (empty result / "0 projects"). This proves the globs parse and match nothing yet. If it errors, the YAML is malformed — fix before continuing.

- [ ] **Step 4: Commit**

```bash
git add .moon/workspace.yml
git commit -m "$(cat <<'EOF'
chore(repo): add Moon workspace.yml and drop .moon/.gitkeep

Project globs (zero matches today), GitHub VCS provider, Moon-owned
CODEOWNERS via codeowners.sync, and generator template registration.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `.moon/toolchain.yml` (resolve current-stable, then pin)

**Files:**
- Create: `.moon/toolchain.yml`

- [ ] **Step 1: Resolve current-stable versions and write the file**

Run this single block (resolution + file write happen in one shell so the values can't drift):
```bash
RUST=$(curl -fsSL https://endoflife.date/api/rust.json \
  | node -e 'const d=JSON.parse(require("fs").readFileSync(0));console.log(d[0].latest)')
NODE=$(curl -fsSL https://nodejs.org/dist/index.json \
  | node -e 'const d=JSON.parse(require("fs").readFileSync(0));console.log(d.find(x=>x.version.startsWith("v22.")).version.slice(1))')
PNPM=$(curl -fsSL https://registry.npmjs.org/pnpm \
  | node -e 'const d=JSON.parse(require("fs").readFileSync(0));console.log(d["dist-tags"].latest)')
PY=$(curl -fsSL https://endoflife.date/api/python.json \
  | node -e 'const d=JSON.parse(require("fs").readFileSync(0));console.log(d.find(x=>x.cycle==="3.12").latest)')
UV=$(curl -fsSL https://api.github.com/repos/astral-sh/uv/releases/latest \
  | node -e 'const d=JSON.parse(require("fs").readFileSync(0));console.log(d.tag_name.replace(/^v/,""))')
echo "Resolved: rust=$RUST node=$NODE pnpm=$PNPM python=$PY uv=$UV"

cat > .moon/toolchain.yml <<EOF
\$schema: 'https://moonrepo.dev/schemas/toolchain.json'

node:
  version: '${NODE}'
  packageManager: 'pnpm'
  pnpm:
    version: '${PNPM}'

rust:
  version: '${RUST}'
  components:
    - 'rustfmt'
    - 'clippy'
  bins:
    - 'cargo-nextest'

# Python/uv are Moon 2.2.5 built-in toolchains keyed 'unstable_python' and the
# separate 'unstable_uv' (verified via 'moon toolchain info'; unprefixed
# 'python'/'uv' are NOT built-in in 2.2.5). uv version pins under 'unstable_uv',
# not nested in 'unstable_python'. Fallback: drop these blocks and run uv via
# plain 'command' tasks per project.
unstable_python:
  version: '${PY}'
  packageManager: 'uv'
unstable_uv:
  version: '${UV}'
EOF
cat .moon/toolchain.yml
```

- [ ] **Step 2: Sanity-check the resolved values against the AC constraints**

Inspect the printed `.moon/toolchain.yml`. Confirm:
- `node.version` starts with `22.` (AC: Node 22.x LTS).
- `unstable_python.version` starts with `3.12.` (AC: Python 3.12.x).
- `rust.version` is a `1.x.y` stable (AC: Rust latest stable).
- `pnpm` is `10.x`, `uv` is `0.11.x` (current majors).

If any constraint is violated (e.g. the Node feed returned a non-22 line), edit the value by hand to the latest patch of the required line before committing. The exact patches are whatever is current today — there is no fixed expected number.

- [ ] **Step 3: Validate Moon accepts the toolchain config**

Run:
```bash
moon query projects
```
Expected: still succeeds with zero projects (this also forces Moon to parse `toolchain.yml`; a schema error here means a bad field/value). Do **not** run a task — that would attempt toolchain downloads, which is unnecessary on the empty workspace.

- [ ] **Step 4: Commit**

```bash
git add .moon/toolchain.yml
git commit -m "$(cat <<'EOF'
chore(repo): pin Rust/Node+pnpm/Python+uv in Moon toolchain.yml

Current-stable versions resolved at build time. Python/uv use Moon 2.2.5's
unstable_python + unstable_uv built-in toolchains with a command-task fallback.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: `.moon/tasks.yml` (global defaults + file groups)

**Files:**
- Create: `.moon/tasks.yml`

- [ ] **Step 1: Write `.moon/tasks.yml`**

Create the file with exactly this content:
```yaml
$schema: 'https://moonrepo.dev/schemas/tasks.json'

fileGroups:
  sources:
    - 'src/**/*'
  tests:
    - 'tests/**/*'
    - '**/*.test.*'
    - '**/*.spec.*'
    - '**/*_test.*'

# Inserted into every inherited task's inputs so a workspace-level toolchain or
# global-task change busts caches. (Caching is on by default, and an undeclared
# task `inputs` already defaults to all project files '**/*', so per-project
# edits invalidate correctly without further config.)
implicitInputs:
  - '/.moon/toolchain.yml'
  - '/.moon/tasks.yml'

taskOptions:
  outputStyle: 'buffer-only-failure'
```

- [ ] **Step 2: Verify Moon still parses cleanly**

Run:
```bash
moon query projects
```
Expected: succeeds, zero projects (forces a parse of `tasks.yml`).

- [ ] **Step 3: Commit**

```bash
git add .moon/tasks.yml
git commit -m "$(cat <<'EOF'
chore(repo): add Moon global tasks.yml (file groups + defaults)

sources/tests file groups, implicitInputs busting caches on toolchain/global
changes, and a buffer-only-failure output style.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Generator templates (one per language)

Three Moon generator templates registered by `workspace.yml`'s `generator.templates`. Each declares a required `name` (Moon has no built-in project-name variable) and an `archetype` enum, and renders a Tera-templated `moon.yml`.

**Files:**
- Create: `.moon/templates/rust/template.yml`
- Create: `.moon/templates/rust/moon.yml`
- Create: `.moon/templates/python/template.yml`
- Create: `.moon/templates/python/moon.yml`
- Create: `.moon/templates/typescript/template.yml`
- Create: `.moon/templates/typescript/moon.yml`

- [ ] **Step 1: Create `.moon/templates/rust/template.yml`**

```yaml
title: 'Rust crate'
description: |
  Scaffolds a moon.yml for a Rust crate in paigasus-core. Choose `library` for
  crates under rs/crates/libs (and bindings), or `service` for binaries under
  rs/crates/services.

  CAVEATS:
  - The `service` archetype emits dependsOn paigasus-proto/paigasus-kernel and a
    `contracts:generate` dep, which only resolve after SMA-357/360. Generate into
    a workspace where those projects exist, or hand-edit the references.
  - The `library` archetype emits NO dependsOn on purpose: most libs depend on
    paigasus-kernel, but paigasus-kernel and paigasus-proto must NOT (self/cycle).
    Add `dependsOn: ['paigasus-kernel']` by hand where appropriate.
variables:
  name:
    type: 'string'
    default: ''
    required: true
    prompt: 'Crate name (e.g. paigasus-kernel)?'
  archetype:
    type: 'enum'
    values: ['library', 'service']
    default: 'library'
    prompt: 'Archetype?'
```

- [ ] **Step 2: Create `.moon/templates/rust/moon.yml`**

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'
type: '{% if archetype == "service" %}application{% else %}library{% endif %}'
language: 'rust'
{% if archetype == "service" %}
dependsOn:
  - 'paigasus-proto'
  - 'paigasus-kernel'
{% endif %}
tasks:
  build:
    command: 'cargo build -p {{ name }}{% if archetype == "service" %} --release{% endif %}'
    inputs: ['@group(sources)', 'Cargo.toml']
{% if archetype == "service" %}
    deps: ['contracts:generate', '^:build']
{% endif %}
  test:
    command: 'cargo nextest run -p {{ name }}'
    inputs: ['@group(sources)', '@group(tests)', 'Cargo.toml']
{% if archetype == "service" %}
    deps: ['contracts:generate']
{% endif %}
  lint:
    command: 'cargo clippy -p {{ name }} -- -D warnings'
    inputs: ['@group(sources)', 'Cargo.toml']
  fmt:
    command: 'cargo fmt --check -p {{ name }}'
    inputs: ['@group(sources)']
```

- [ ] **Step 3: Create `.moon/templates/python/template.yml`**

```yaml
title: 'Python package'
description: |
  Scaffolds a moon.yml for a Python package under py/packages. `library` for an
  importable package, `service` for a runnable service.

  CAVEAT: a package that builds native code via maturin must add
  `dependsOn: ['<rust-binding-crate>']` by hand so Moon provisions BOTH the
  Python and Rust toolchains in the task context (not exercised yet; first
  surfaces in the kernel-bindings work).
variables:
  name:
    type: 'string'
    default: ''
    required: true
    prompt: 'Package name (e.g. paigasus-ml)?'
  archetype:
    type: 'enum'
    values: ['library', 'service']
    default: 'library'
    prompt: 'Archetype?'
```

- [ ] **Step 4: Create `.moon/templates/python/moon.yml`**

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'
type: '{% if archetype == "service" %}application{% else %}library{% endif %}'
language: 'python'
tasks:
  build:
    command: 'uv build'
    inputs: ['@group(sources)', 'pyproject.toml']
    outputs: ['dist']
  lint:
    command: 'ruff check .'
    inputs: ['@group(sources)', '@group(tests)']
  format:
    command: 'ruff format --check .'
    inputs: ['@group(sources)', '@group(tests)']
  typecheck:
    command: 'basedpyright'
    inputs: ['@group(sources)', '@group(tests)']
  test:
    command: 'pytest'
    inputs: ['@group(sources)', '@group(tests)']
{% if archetype == "service" %}
  start:
    command: 'python -m {{ name | replace(from="-", to="_") }}'
    options:
      cache: false
      persistent: true
{% endif %}
```

- [ ] **Step 5: Create `.moon/templates/typescript/template.yml`**

```yaml
title: 'TypeScript project'
description: |
  Scaffolds a moon.yml for a TypeScript project. `library` for a publishable
  package under ts/packages, `app` for a deployable under ts/apps (e.g. Next.js).
  Lint/format use ESLint + Prettier per ADR-0009; the test runner is a scaffold
  default (vitest) — finalize in SMA-359.
variables:
  name:
    type: 'string'
    default: ''
    required: true
    prompt: 'Project name (e.g. sdk)?'
  archetype:
    type: 'enum'
    values: ['library', 'app']
    default: 'library'
    prompt: 'Archetype?'
```

- [ ] **Step 6: Create `.moon/templates/typescript/moon.yml`**

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'
type: '{% if archetype == "app" %}application{% else %}library{% endif %}'
language: 'typescript'
tasks:
  build:
    command: '{% if archetype == "app" %}next build{% else %}tsc --build{% endif %}'
    inputs: ['@group(sources)', 'tsconfig.json', 'package.json']
    outputs: ['{% if archetype == "app" %}.next{% else %}dist{% endif %}']
  lint:
    command: 'eslint .'
    inputs: ['@group(sources)']
  format:
    command: 'prettier --check .'
    inputs: ['@group(sources)', '@group(tests)']
  test:
    command: 'vitest run'
    inputs: ['@group(sources)', '@group(tests)']
```

- [ ] **Step 7: Verify each template renders syntactically valid YAML (dry run, both archetypes)**

Run (renders to a temp dir, parses the output with `node`, then cleans up):
```bash
TMP=$(mktemp -d)
set -e
for spec in "rust library" "rust service" "python library" "python service" "typescript library" "typescript app"; do
  set -- $spec; tmpl=$1; arch=$2
  out="$TMP/$tmpl-$arch"
  moon generate "$tmpl" --to "$out" --defaults --force -- --name "demo-$tmpl" --archetype "$arch"
  node -e 'const fs=require("fs");const f=process.argv[1];const s=fs.readFileSync(f,"utf8");if(!/^type:\s*.+/m.test(s)){throw new Error("no type in "+f)};console.log("OK "+f)' "$out/moon.yml"
done
rm -rf "$TMP"
echo "all templates rendered + parsed"
```
Expected: six `OK .../moon.yml` lines then `all templates rendered + parsed`. The check asserts each rendered file has a `type:` line; eyeball one service render to confirm the `deps:`/`dependsOn:` blocks are correctly indented. If `moon generate`'s non-interactive flag form differs in the pinned Moon version, run `moon generate --help` and adjust the `--defaults`/`-- --name` invocation accordingly (the templates themselves do not change).

- [ ] **Step 8: Commit**

```bash
git add .moon/templates
git commit -m "$(cat <<'EOF'
chore(repo): add per-language Moon generator templates

rust (library|service), python (library|service), typescript (library|app),
each an archetype-parameterized moon generate scaffold. Reference docs for
SMA-357/358/359.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: CODEOWNERS — generate via Moon, reconcile the static file

The static root `CODEOWNERS` (`* @SMK1085`) from SMA-355 must be replaced by Moon's generated file. Whether `globalPaths` alone (zero projects) emits a file is unconfirmed — so verify before deleting.

**Files:**
- Create (generated): `.github/CODEOWNERS`
- Delete (conditionally): `CODEOWNERS` (root)

- [ ] **Step 1: Trigger CODEOWNERS sync**

Run:
```bash
moon sync code-owners
echo "--- .github/CODEOWNERS ---"; cat .github/CODEOWNERS 2>/dev/null || echo "(none at .github/)"
echo "--- root CODEOWNERS ---";   cat CODEOWNERS 2>/dev/null || echo "(none at root)"
```
Expected (GitHub provider): Moon writes `.github/CODEOWNERS` containing a line that maps `*` to `@SMK1085`.

- [ ] **Step 2: Branch on the result**

**Case A — `.github/CODEOWNERS` exists and contains `@SMK1085`:** remove the redundant static root file (it's superseded by the generated one).
```bash
test -f .github/CODEOWNERS && grep -q '@SMK1085' .github/CODEOWNERS && git rm CODEOWNERS
```

**Case B — no file was generated (Moon emits nothing for a zero-project workspace):** keep the static root `CODEOWNERS` so the repo never has zero owners files, and record the deferral. Run:
```bash
test -f .github/CODEOWNERS || cat >> CONTRIBUTING.md <<'EOF'

> **Note:** `codeowners.sync` is enabled in `.moon/workspace.yml`, but Moon does
> not emit a `CODEOWNERS` until projects with `owners` exist. Until then the
> static root `CODEOWNERS` (`* @SMK1085`) stands in; SMA-363 verifies the synced
> file once Phase-1 projects land.
EOF
```
Proceed with whichever case applies; do not do both.

- [ ] **Step 3: Confirm exactly one CODEOWNERS file is authoritative**

Run:
```bash
echo "tracked CODEOWNERS files:"; git ls-files | grep -E '(^|/)CODEOWNERS$' || true
git status --porcelain
```
Expected: in Case A, only `.github/CODEOWNERS` remains tracked (root `CODEOWNERS` staged for deletion). In Case B, only the root `CODEOWNERS` exists. Never both as live owners files.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
chore(repo): let Moon own CODEOWNERS via codeowners.sync

Generates .github/CODEOWNERS from the workspace global owner and removes the
static root file (or defers if Moon emits nothing on the empty workspace).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: CONTRIBUTING — "Local development setup"

Document the `proto` → `moon` install order so a fresh clone can reach a working `moon` (feeds SMA-363's fresh-clone gate; SMA-371 later extends this same subsection with lefthook).

**Files:**
- Modify: `CONTRIBUTING.md`

- [ ] **Step 1: Insert the subsection before the existing "## Commit messages" heading**

Read `CONTRIBUTING.md`, find the `## Commit messages` heading, and insert this block immediately before it:
````markdown
## Local development setup

Tooling is orchestrated by [Moon](https://moonrepo.dev), and Moon itself is
version-pinned via [proto](https://moonrepo.dev/proto) in `.prototools`. One-time
setup:

```bash
# 1. Install proto (toolchain manager)
bash <(curl -fsSL https://moonrepo.dev/install/proto.sh) --yes
#    add proto to your shell PATH if the installer didn't (see its output)

# 2. Install the pinned Moon binary from .prototools
proto install

# 3. Verify
moon --version
```

Moon downloads and pins the per-language toolchains (Rust, Node + pnpm, Python +
uv) from `.moon/toolchain.yml` on first use — no manual language installs needed.

> Output is buffered for passing tasks (`buffer-only-failure`). To watch a long
> task stream locally, append `--output-style stream`, e.g.
> `moon run <project>:test --output-style stream`.

````

- [ ] **Step 2: Verify the subsection is present and well-formed**

Run:
```bash
grep -q '## Local development setup' CONTRIBUTING.md \
  && grep -q 'proto install' CONTRIBUTING.md \
  && grep -q 'output-style stream' CONTRIBUTING.md \
  && echo "OK: subsection present"
```
Expected: `OK: subsection present`.

- [ ] **Step 3: Commit**

```bash
git add CONTRIBUTING.md
git commit -m "$(cat <<'EOF'
docs: document proto -> moon local setup in CONTRIBUTING

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Verify AC gates, then push + open PR

**Files:** none created — verification and PR only.

- [ ] **Step 1: `moon ci :build` runs clean on the empty workspace (AC gate 1)**

Run:
```bash
moon ci :build
```
Expected: completes with no project errors and no tasks run (zero affected targets). (Moon 2.x: `moon ci` takes explicit `[TARGETS]`; bare `moon ci` errors `app::tty::required_id` in non-TTY.) If Moon attempts to provision the `unstable_python`/`unstable_uv` toolchains and fails despite there being no tasks, apply the spec's fallback (drop those blocks from `.moon/toolchain.yml` and run uv via command tasks) and record it — but the expected result is a clean no-op.

- [ ] **Step 2: `moon ci :test` succeeds as a no-op (AC gate 2)**

Run:
```bash
moon ci :test
```
Expected: succeeds with nothing to run (no `test` tasks exist because no real projects exist). (Moon 2.x: `moon check` takes project IDs, not task targets, so `moon check :build` is invalid — use explicit `moon ci <target>`. SMA-363's AC was updated to this `moon ci :build`/`:test` form.)

- [ ] **Step 3: Confirm zero projects and a clean tree**

Run:
```bash
moon query projects          # expect zero projects
git status --porcelain       # expect empty (clean)
test ! -f .moon/.gitkeep && echo "OK: .gitkeep removed"
git ls-files .moon | grep -E '\.moon/(cache|docker)/' && echo "FAIL: cache tracked" || echo "OK: no moon cache tracked"
```
Expected: zero projects; clean status; `OK: .gitkeep removed`; `OK: no moon cache tracked`.

- [ ] **Step 4: Push the branch**

Run:
```bash
git push -u origin feature/sma-356-set-up-moon-configuration
```

- [ ] **Step 5: Open the PR to `main`**

Run:
```bash
gh pr create --base main \
  --title "SMA-356: Set up Moon configuration" \
  --body "$(cat <<'EOF'
## Summary

Wires Moon as the polyglot task orchestrator: `.moon/workspace.yml` (project
globs, GitHub VCS provider, Moon-owned CODEOWNERS via `codeowners.sync`,
generator registration), `.moon/toolchain.yml` (current-stable Rust / Node+pnpm /
Python+uv pins), `.moon/tasks.yml` (file groups + global defaults), per-language
`moon generate` templates (rust/python/typescript, archetype-parameterized), and
`.prototools` pinning the Moon binary. The project globs match zero directories
today, so `moon ci :build` is a clean no-op.

## Acceptance criteria

- [x] `.moon/workspace.yml` with the seven project globs
- [x] `.moon/toolchain.yml` pinning Rust (+rustfmt/clippy/cargo-nextest), Node 22.x LTS (+pnpm), Python 3.12.x (+uv, via `unstable_python`/`unstable_uv`)
- [x] `.moon/tasks.yml` with global defaults + `sources`/`tests` file groups
- [x] Per-project `moon.yml` templates (library/service/app) as `moon generate` scaffolds
- [x] `codeowners.sync: true` (corrected from the AC's invalid `syncOnRun`)
- [x] `moon ci :build` runs cleanly on the empty workspace
- [x] `moon ci :test` succeeds across all language workspaces (no-op on empty)

Design: docs/superpowers/specs/2026-05-26-moon-configuration-design.md
Plan: docs/superpowers/plans/2026-05-26-moon-configuration.md

> Note: the AC field name `codeowners.syncOnRun` was corrected to `codeowners.sync`
> (the valid current-Moon field) in Linear and the Notion scoping doc.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```
Expected: `gh` prints the new PR URL.

- [ ] **Step 6: Report the PR URL** for review before merge.

---

## Notes for the implementer

- **Do not** create any real projects (`Cargo.toml`, `pyproject.toml`,
  `pnpm-workspace.yaml`, `buf.yaml`, crate/package directories) or CI workflow
  files — those belong to SMA-357–361 and are out of scope (spec "Out of scope").
- **Versions are not fixed numbers.** Tasks 1 and 3 resolve today's latest stable
  at run time. The only hard constraints are Node `22.x` and Python `3.12.x`
  (verify in Task 3 Step 2); everything else is "latest stable / current major".
- **If `gh` is not authenticated**, stop at Step 4 and report; the user can open
  the PR manually or run `gh auth login`.
- **`repo` project + `vcs.hooks`** are deliberately absent — SMA-371 adds them.
- If `moon generate`'s non-interactive flags differ in the pinned Moon version,
  the fix is the *invocation* in Task 5 Step 7, never the template files.

## Post-implementation reconciliation (Moon 2.2.5)

Executed in PR #2. Deltas from this plan's 1.x-era assumptions (full detail in the
spec's "Post-implementation outcomes (Moon 2.2.5)"):

- Moon pinned to **2.2.5**; `vcs.manager` → `vcs.client`; Python/uv use the
  `unstable_python` + `unstable_uv` built-in toolchains (verified via
  `moon toolchain info`; unprefixed `python`/`uv` are not built-in in 2.2.5, and uv
  version pins under `unstable_uv`); sync subcommand is `moon sync code-owners`.
- Moon resolves **one task-less project** (`contracts`), not zero; the language
  globs match nothing yet.
- Verified gate is **`moon ci :build`** (exit 0) — bare `moon ci` needs explicit
  targets and `moon check :build` is invalid in Moon 2.x. CODEOWNERS generated at
  `.github/CODEOWNERS` from `globalPaths` alone (Case A); static root file removed.
- Resolved pins: Rust 1.95.0 / Node 22.22.3 / pnpm 11.3.0 / Python 3.12.13 /
  uv 0.11.16.
- `moon generate` non-interactive form: `moon generate <tmpl> --to <dir> --defaults
  --force -- --name <name> --archetype <a>` (a destination INSIDE the workspace;
  an absolute out-of-tree `--to` is treated as workspace-relative).
