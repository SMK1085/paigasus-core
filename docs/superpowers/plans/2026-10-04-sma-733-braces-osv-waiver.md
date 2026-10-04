# SMA-733 braces osv waiver and shipped-path guard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Waive GHSA-vfj7-8cjw-p6xm (`braces@3.0.3`) in `osv-scanner.toml` until 2027-01-02, and add a guard that fails `repo:osv` when a shipped importer reaches `braces`.

**Architecture:** A new Python script, `ci/osv/shipped_reachability.py`, reads `ts/pnpm-lock.yaml` with a line parser (standard library only). It walks the snapshot edges from the shipped importers (`apps/*` and the workspace packages that they link) and from every importer. `ci/osv/run.sh` runs the script twice (`--self-test`, then the real files), maps its exit codes, and always prints the osv findings.

**Tech Stack:** Python 3.9+ standard library, bash (3.2 and 5 compatible), osv-scanner 2.5.0, pnpm lock file format 9.0.

**Spec:** `docs/superpowers/specs/2026-10-04-sma-733-braces-osv-waiver-design.md`

## Prototype record

The planner built and ran every file in this plan in a scratch copy before it wrote the plan. The code below is the code that ran. These are the measured results, on 2026-10-04, at `7eae5320`:

| Run | Result |
|---|---|
| Guard on the real `ts/pnpm-lock.yaml` and the new `osv-scanner.toml`, `/usr/bin/python3` 3.9.6 and Homebrew 3.14.7 | rc 0, one line: `braces (GHSA-vfj7-8cjw-p6xm) reached only from dev tooling, not from a shipped importer` |
| Guard on the real lock file and the CURRENT `osv-scanner.toml` (no waiver) | rc 2, `table-stale` |
| Real lock file shape | 13 importers, 1129 snapshots, 2485 snapshot edges, 28 `link:` edges. Every `link:` resolved |
| Shipped importers | `apps/gateway-console`, `apps/iam-console` and 9 packages. Only `.` and `packages/commitlint-config` are not shipped |
| Control path to `braces` (3 edges) | `. > @semantic-release/commit-analyzer@13.0.1(semantic-release@25.0.9(supports-color@7.2.0)(typescript@6.0.3))(supports-color@7.2.0) > micromatch@4.0.8 > braces@3.0.3` |
| Importers that reach `braces` (full walk from each importer) | only `.` |
| Shipped walk | reaches `brace-expansion@5.0.12` and `brace-expansion@2.1.7`, not `braces`. The name match must be exact |
| `--self-test`, both pythons | rc 0, `13 fixtures passed` |
| `ruff check --config py/pyproject.toml` (ruff 0.16.9) | clean. Without the `# noqa: UP036` comment, ruff reports UP036 |
| New `run.sh`, `/bin/bash` 3.2.57 and Homebrew bash 5.3.15 | rc 0 under both |

Fixture results (the first output line holds the reason token):

| Fixture | rc | Token |
|---|---|---|
| `clean.yaml`, `block-end.yaml`, `catalogs.yaml` | 0 | none |
| `shipped-prod.yaml`, `shipped-dev.yaml`, `shipped-link.yaml`, `shipped-optional.yaml`, `shipped-alias.yaml`, `file-dep.yaml` | 3 | `shipped-path` |
| `bad-version.yaml` | 2 | `lockfile-version` |
| `dangling.yaml` | 2 | `dangling-edge` |
| `no-edges.yaml` | 2 | `edge-control` |
| `table-stale.yaml` (with `table-stale.toml`) | 2 | `table-stale` |

Mutation results:

| Mutation | Spec prediction | Measured |
|---|---|---|
| M1a: guard call on `fixtures/shipped-prod.yaml` | `run.sh` exits 1 | rc 1 |
| M1b: guard calls deleted | `run.sh` exits 0 | rc 0 (the residual of spec section 4.5) |
| M2: no `devDependencies` | `shipped-dev.yaml` fails | `shipped-dev.yaml` fails (rc 2 `edge-control`, expected 3). `block-end.yaml`, `catalogs.yaml` and the real run also fail with `edge-control` |
| M3: no snapshot edges | real run exits 2 `edge-control` | rc 2 `edge-control`. 10 fixtures fail |
| M4: no dangling-edge check | `dangling.yaml` fails | `dangling.yaml` gives rc 0. Real run stays 0 |
| M5: no `optionalDependencies` | `shipped-optional.yaml` fails | it gives rc 2 `edge-control`. Real run stays 0, so only the self-test catches M5. `run.sh` exits 2 |
| M6: alias as `name@value` | `shipped-alias.yaml` fails | it gives rc 2 `dangling-edge`. Real run also gives rc 2 `dangling-edge` (`string-width-cjs`) |
| M7: no `python3` on `PATH` | `run.sh` exits 2 | rc 2 (both guard calls exit 127) |
| M8: the guard raises | `run.sh` exits 2 | guard rc 1, `run.sh` rc 2 |
| Waiver removed (spec section 5 step 3) | osv finds the GHSA, guard `table-stale`, `run.sh` exits 2 | as predicted. Both results print |

### Deviations from the spec found while prototyping

No spec prediction was false. These points add to the spec or make it exact. None changes the design.

1. **M2 has a wider effect than the spec says.** The spec predicts that `shipped-dev.yaml` fails. It does. The shipped walk and the control walk use the same importer edges, so M2 also removes the root importer's only path to `braces`. Thus three more fixtures and the real run fail with `edge-control`.
2. **M5 is caught only by the self-test.** The real braces path uses no `optionalDependencies` edge. `run.sh` still exits 2, because a self-test failure maps to guard=2.
3. **A trap in the M1 mutation.** The first M1 attempt put the `# MUTATION-M1` marker in the middle of the line. That made `|| guard_rc=$?` part of the comment, and `run.sh` exited 0 although the guard printed `shipped-path`. Put a marker only at the END of a line. Task 3 does this.
4. **`file:` values are relative to the lock-file directory, not to the importer.** `packages/paigasus-kernel` has `specifier: file:../../../rs/...` but `version: file:../rs/...`. The snapshot key is `name@` plus the `version:` value. The spec's rule (`name@file:...`, verbatim) is therefore correct. No path resolution is necessary for `file:`.
5. **Additions that the spec does not name:** (a) `EXPECTED_FIXTURES`, a tuple of the 13 fixture names. The self-test fails when the fixture set changes, so a lost fixture cannot make the self-test pass. (b) A fixture names its expectation in a header line `# expect: <rc> [token]`, and its config in an optional `# config: <file>` line. The default config is `fixtures/waiver-config.toml`. The spec's "`table-stale` case" is the fixture `table-stale.yaml` with `fixtures/table-stale.toml`. (c) The self-test catches an exception per fixture and names the fixture. (d) The walk uses `graph.get(node, [])`, so that M4 gives a clean mismatch, not a `KeyError`. (e) Ruff targets py312, so the 3.9 floor check needs `# noqa: UP036`.
6. **F6 was not re-measured in this worktree.** The worktree has no `.next` output. The main checkout's older `.next` output has no `braces`, `micromatch` or `fast-glob` directory and no `braces@3`, `node_modules/braces` or `/braces/lib` string. Task 3 builds both apps and measures again.

## Global Constraints

- The guard uses the Python 3 standard library only. It runs on Python 3.9 or later (`/usr/bin/python3` 3.9.6 and Homebrew 3.14.7). At start it checks `sys.version_info >= (3, 9)`, else it exits 2 with `python-too-old`.
- Guard exit codes: 0 pass, 2 infrastructure (reason token in the first output line), 3 `shipped-path`. A traceback exits 1.
- `run.sh` maps: 0 pass; 3 finding (`guard=1`); every other code (1, 2, 126, 127, a signal) is `guard=2`. Exit precedence: 2 if `guard` is 2; else 1 if osv found something or `guard` is 1; else 0.
- `SHIPPED_FREE_WAIVERS = {"GHSA-vfj7-8cjw-p6xm": "braces"}`.
- `ignoreUntil = 2027-01-02`, a bare TOML date. A quoted date is a config error.
- The `reason` is one line, exactly: `No fixed braces release. Only root dev tooling (ESLint Next plugin, semantic-release) reaches it, on repo-authored glob patterns; no shipped package does, and ci/osv/shipped_reachability.py enforces that (SMA-733).`
- Every new `.py`, `.yaml` and `.toml` file starts with `# SPDX-License-Identifier: Apache-2.0`. The fixture YAML in `ci/helm-render/fixtures/` has this header too.
- `run.sh` uses no `mapfile` and no `declare -A`. It must run under `/bin/bash` 3.2 and bash 5.
- No change to `moon.yml` or to any workflow. `repo:osv` inputs already include `ci/osv/**/*`.
- Every new `ci/**/*.py` file must pass `repo:ruff-ci` (`py/pyproject.toml` rules, target py312).
- Rules for implementers: do not use `git commit --amend`. Do not use `git reset`. Run every command in the foreground. Do not install host software (no `brew`). Do not use `git checkout --` to undo a mutation.
- Commit messages: conventional, scope `ci`. The last line is `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. The body has no `#NNN` and no line of the form `word: value`.
- Before a `python3`, `moon`, `uv` or `osv-scanner` command, run `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text`.

## Review Focus

1. A shipped app reaches a package whose name only starts with `braces` (the real lock file has `brace-expansion` on shipped paths). The guard must pass. Pinned by `clean.yaml` (Task 1).
2. The config names the waived ID only in a comment (the `osv-scanner.toml` header has an example `id = ...` line in a comment). The guard must say `table-stale`. Pinned by `table-stale.toml` (Task 1).
3. A workspace package that only the root importer links (`packages/commitlint-config`) reaches `braces`. That package does not ship, so the guard must pass. Pinned by `clean.yaml` (Task 1).
4. A package links a sibling with a single `../` (`link:../paigasus-ui`), and that sibling reaches `braces`. The guard must resolve the link relative to the package and fail. Pinned by `shipped-link.yaml` (Task 1).
5. The guard runs from a different working directory (a developer runs it by hand). The defaults and the fixture paths must not depend on the working directory. Pinned by Task 1 Step 7.

---

### Task 1: The guard script, its fixtures and its self-test

**Files:**
- Create: `ci/osv/shipped_reachability.py`
- Create: `ci/osv/fixtures/` with 13 `.yaml` files and 2 `.toml` files (listed in Step 1)

**Interfaces:**
- Consumes: nothing.
- Produces: the command line `python3 ci/osv/shipped_reachability.py [--lockfile PATH] [--config PATH] | --self-test`. The defaults are `<repo>/ts/pnpm-lock.yaml` and `<repo>/osv-scanner.toml`. Exit codes 0, 2, 3 (a traceback is 1). `--self-test` exits 0 or 2. The module constant `SHIPPED_FREE_WAIVERS` and the functions `check(lockfile: Path, config: Path) -> tuple[int, list[str]]` and `parse_lockfile(text: str) -> tuple[str | None, dict, dict]`. Task 3 mutates the lines named there.

- [ ] **Step 1: Write the fixtures**

Make the directory `ci/osv/fixtures/`. Write these 15 files exactly. Each lock-file fixture uses real names, quoting and peer suffixes from `ts/pnpm-lock.yaml`. Each has a scoped `@` name and a `(peer)` suffix. The comment lines at the top are part of the test: the parser must skip them.

`ci/osv/fixtures/waiver-config.toml`:

```toml
# SPDX-License-Identifier: Apache-2.0
# Fixture config for ci/osv/shipped_reachability.py --self-test (SMA-733). Every fixture
# without a `# config:` line uses this file.
[[IgnoredVulns]]
id = "GHSA-vfj7-8cjw-p6xm"
ignoreUntil = 2027-01-02
reason = "Fixture only."
```

`ci/osv/fixtures/table-stale.toml`:

```toml
# SPDX-License-Identifier: Apache-2.0
# Fixture config for ci/osv/shipped_reachability.py --self-test (SMA-733). It names
# GHSA-vfj7-8cjw-p6xm only in a comment, which must not count as a waiver:
#   id = "GHSA-vfj7-8cjw-p6xm"
[[IgnoredVulns]]
id = "GHSA-xxxx-xxxx-xxxx"
ignoreUntil = 2027-01-02
reason = "Fixture only."
```

`ci/osv/fixtures/clean.yaml`:

```yaml
# SPDX-License-Identifier: Apache-2.0
# expect: 0
# braces is reached only from the root importer's devDependencies, through three snapshot
# edges, and from packages/commitlint-config, which only `.` links. A shipped app reaches
# brace-expansion, a different package: the name match must be exact.
lockfileVersion: '9.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

importers:

  .:
    devDependencies:
      '@next/eslint-plugin-next':
        specifier: 'catalog:'
        version: 16.3.6(eslint@10.11.0(jiti@2.7.0)(supports-color@7.2.0))
      '@paigasus/commitlint-config':
        specifier: workspace:*
        version: link:packages/commitlint-config

  apps/iam-console:
    dependencies:
      '@paigasus/ui':
        specifier: workspace:*
        version: link:../../packages/paigasus-ui
    devDependencies:
      minimatch:
        specifier: 'catalog:'
        version: 10.2.6

  packages/commitlint-config:
    dependencies:
      '@semantic-release/commit-analyzer':
        specifier: 'catalog:'
        version: 13.0.1(semantic-release@25.0.9(supports-color@7.2.0)(typescript@6.0.3))(supports-color@7.2.0)

  packages/paigasus-ui:
    dependencies:
      clsx:
        specifier: 'catalog:'
        version: 2.1.1

packages:

  braces@3.0.3:
    resolution: {integrity: sha512-yQbXgO/OSZVD2IsiLlro+7Hf6Q18EJrKSEsdoMzKePKXct3gvD8oLcOQdIzGupr5Fj+EDe8gO/lxc1BzfMpxvA==}
    engines: {node: '>=8'}

snapshots:

  '@next/eslint-plugin-next@16.3.6(eslint@10.11.0(jiti@2.7.0)(supports-color@7.2.0))':
    dependencies:
      fast-glob: 3.3.1
    transitivePeerDependencies:
      - eslint

  '@semantic-release/commit-analyzer@13.0.1(semantic-release@25.0.9(supports-color@7.2.0)(typescript@6.0.3))(supports-color@7.2.0)':
    dependencies:
      micromatch: 4.0.8
    transitivePeerDependencies:
      - supports-color

  balanced-match@4.0.4: {}

  brace-expansion@5.0.12:
    dependencies:
      balanced-match: 4.0.4

  braces@3.0.3:
    dependencies:
      fill-range: 7.1.1

  clsx@2.1.1: {}

  fast-glob@3.3.1:
    dependencies:
      micromatch: 4.0.8

  fill-range@7.1.1: {}

  micromatch@4.0.8:
    dependencies:
      braces: 3.0.3
      picomatch: 2.3.2

  minimatch@10.2.6:
    dependencies:
      brace-expansion: 5.0.12

  picomatch@2.3.2: {}
```

`ci/osv/fixtures/shipped-prod.yaml`:

```yaml
# SPDX-License-Identifier: Apache-2.0
# expect: 3 shipped-path
# An app's production dependency reaches braces through two snapshot edges.
lockfileVersion: '9.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

importers:

  .:
    devDependencies:
      typescript:
        specifier: 'catalog:'
        version: 6.0.3

  apps/iam-console:
    dependencies:
      '@semantic-release/commit-analyzer':
        specifier: 'catalog:'
        version: 13.0.1(semantic-release@25.0.9(supports-color@7.2.0)(typescript@6.0.3))(supports-color@7.2.0)

snapshots:

  '@semantic-release/commit-analyzer@13.0.1(semantic-release@25.0.9(supports-color@7.2.0)(typescript@6.0.3))(supports-color@7.2.0)':
    dependencies:
      micromatch: 4.0.8
    transitivePeerDependencies:
      - supports-color

  braces@3.0.3:
    dependencies:
      fill-range: 7.1.1

  fill-range@7.1.1: {}

  micromatch@4.0.8:
    dependencies:
      braces: 3.0.3
      picomatch: 2.3.2

  picomatch@2.3.2: {}

  typescript@6.0.3: {}
```

`ci/osv/fixtures/shipped-dev.yaml`:

```yaml
# SPDX-License-Identifier: Apache-2.0
# expect: 3 shipped-path
# An app's devDependency reaches braces. ts/Dockerfile installs dev dependencies, so they ship.
lockfileVersion: '9.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

importers:

  .:
    devDependencies:
      typescript:
        specifier: 'catalog:'
        version: 6.0.3

  apps/gateway-console:
    dependencies:
      '@connectrpc/connect':
        specifier: 'catalog:'
        version: 2.2.0(@bufbuild/protobuf@2.15.0)
    devDependencies:
      '@next/eslint-plugin-next':
        specifier: 'catalog:'
        version: 16.3.6(eslint@10.11.0(jiti@2.7.0)(supports-color@7.2.0))

snapshots:

  '@connectrpc/connect@2.2.0(@bufbuild/protobuf@2.15.0)':
    dependencies:
      '@bufbuild/protobuf': 2.15.0

  '@bufbuild/protobuf@2.15.0': {}

  '@next/eslint-plugin-next@16.3.6(eslint@10.11.0(jiti@2.7.0)(supports-color@7.2.0))':
    dependencies:
      fast-glob: 3.3.1
    transitivePeerDependencies:
      - eslint

  braces@3.0.3:
    dependencies:
      fill-range: 7.1.1

  fast-glob@3.3.1:
    dependencies:
      micromatch: 4.0.8

  fill-range@7.1.1: {}

  micromatch@4.0.8:
    dependencies:
      braces: 3.0.3
      picomatch: 2.3.2

  picomatch@2.3.2: {}

  typescript@6.0.3: {}
```

`ci/osv/fixtures/shipped-link.yaml`:

```yaml
# SPDX-License-Identifier: Apache-2.0
# expect: 3 shipped-path
# An app links packages/paigasus-app-shell, which links ../paigasus-ui, whose production
# dependency reaches braces. Both link forms must resolve relative to their own importer.
lockfileVersion: '9.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

importers:

  .:
    devDependencies:
      typescript:
        specifier: 'catalog:'
        version: 6.0.3

  apps/iam-console:
    dependencies:
      '@paigasus/app-shell':
        specifier: workspace:*
        version: link:../../packages/paigasus-app-shell

  packages/paigasus-app-shell:
    dependencies:
      '@paigasus/ui':
        specifier: workspace:*
        version: link:../paigasus-ui

  packages/paigasus-ui:
    dependencies:
      '@semantic-release/commit-analyzer':
        specifier: 'catalog:'
        version: 13.0.1(semantic-release@25.0.9(supports-color@7.2.0)(typescript@6.0.3))(supports-color@7.2.0)

snapshots:

  '@semantic-release/commit-analyzer@13.0.1(semantic-release@25.0.9(supports-color@7.2.0)(typescript@6.0.3))(supports-color@7.2.0)':
    dependencies:
      micromatch: 4.0.8
    transitivePeerDependencies:
      - supports-color

  braces@3.0.3:
    dependencies:
      fill-range: 7.1.1

  fill-range@7.1.1: {}

  micromatch@4.0.8:
    dependencies:
      braces: 3.0.3
      picomatch: 2.3.2

  picomatch@2.3.2: {}

  typescript@6.0.3: {}
```

`ci/osv/fixtures/shipped-optional.yaml`:

```yaml
# SPDX-License-Identifier: Apache-2.0
# expect: 3 shipped-path
# The only path to braces uses an optionalDependencies block in a snapshot.
lockfileVersion: '9.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

importers:

  .:
    devDependencies:
      typescript:
        specifier: 'catalog:'
        version: 6.0.3

  apps/iam-console:
    dependencies:
      '@csstools/css-syntax-patches-for-csstree':
        specifier: 'catalog:'
        version: 1.1.14(css-tree@3.2.1)

snapshots:

  '@csstools/css-syntax-patches-for-csstree@1.1.14(css-tree@3.2.1)':
    optionalDependencies:
      micromatch: 4.0.8

  braces@3.0.3:
    dependencies:
      fill-range: 7.1.1

  fill-range@7.1.1: {}

  micromatch@4.0.8:
    dependencies:
      braces: 3.0.3
      picomatch: 2.3.2

  picomatch@2.3.2: {}

  typescript@6.0.3: {}
```

`ci/osv/fixtures/shipped-alias.yaml`:

```yaml
# SPDX-License-Identifier: Apache-2.0
# expect: 3 shipped-path
# The only path to braces uses an alias value (`micromatch-cjs: micromatch@4.0.8`), in the
# shape of the real `string-width-cjs: string-width@4.2.3` entry under '@isaacs/cliui@8.0.2'.
lockfileVersion: '9.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

importers:

  .:
    devDependencies:
      typescript:
        specifier: 'catalog:'
        version: 6.0.3

  apps/iam-console:
    dependencies:
      '@inquirer/type':
        specifier: 'catalog:'
        version: 4.1.1(@types/node@24.13.6)
      '@isaacs/cliui':
        specifier: 'catalog:'
        version: 8.0.2

snapshots:

  '@inquirer/type@4.1.1(@types/node@24.13.6)':
    optionalDependencies:
      '@types/node': 24.13.6

  '@isaacs/cliui@8.0.2':
    dependencies:
      micromatch-cjs: micromatch@4.0.8
      string-width: 5.1.2

  '@types/node@24.13.6': {}

  braces@3.0.3:
    dependencies:
      fill-range: 7.1.1

  fill-range@7.1.1: {}

  micromatch@4.0.8:
    dependencies:
      braces: 3.0.3
      picomatch: 2.3.2

  picomatch@2.3.2: {}

  string-width@5.1.2: {}

  typescript@6.0.3: {}
```

`ci/osv/fixtures/block-end.yaml`:

```yaml
# SPDX-License-Identifier: Apache-2.0
# expect: 0
# A transitivePeerDependencies: list with quoted items, and an `optional: true` snapshot, sit
# between edge blocks. Neither gives edges, so the app must not reach braces through them.
# Only `.` reaches braces, through three snapshot edges.
lockfileVersion: '9.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

importers:

  .:
    devDependencies:
      '@next/eslint-plugin-next':
        specifier: 'catalog:'
        version: 16.3.6(eslint@10.11.0(jiti@2.7.0)(supports-color@7.2.0))

  apps/iam-console:
    dependencies:
      '@conventional-changelog/git-client':
        specifier: 'catalog:'
        version: 3.1.2(conventional-commits-parser@7.1.2)

snapshots:

  '@colors/colors@1.5.0':
    optional: true

  '@conventional-changelog/git-client@3.1.2(conventional-commits-parser@7.1.2)':
    dependencies:
      '@simple-libs/child-process-utils': 2.0.0
    transitivePeerDependencies:
      - '@types/node'
      - 'braces'
      - supports-color
    optionalDependencies:
      '@colors/colors': 1.5.0

  '@next/eslint-plugin-next@16.3.6(eslint@10.11.0(jiti@2.7.0)(supports-color@7.2.0))':
    dependencies:
      fast-glob: 3.3.1
    transitivePeerDependencies:
      - eslint

  '@simple-libs/child-process-utils@2.0.0': {}

  braces@3.0.3:
    dependencies:
      fill-range: 7.1.1

  fast-glob@3.3.1:
    dependencies:
      micromatch: 4.0.8

  fill-range@7.1.1: {}

  micromatch@4.0.8:
    dependencies:
      braces: 3.0.3
      picomatch: 2.3.2

  picomatch@2.3.2: {}
```

`ci/osv/fixtures/catalogs.yaml`:

```yaml
# SPDX-License-Identifier: Apache-2.0
# expect: 0
# A catalogs: block holds importer-like `version:` lines that name braces, and an overrides:
# block names it too. The guard reads neither section. Only `.` reaches braces.
lockfileVersion: '9.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

catalogs:
  default:
    '@next/eslint-plugin-next':
      specifier: ^16.3.6
      version: 16.3.6
    braces:
      specifier: ^3.0.3
      version: 3.0.3
    micromatch:
      specifier: ^4.0.8
      version: 4.0.8

overrides:
  braces@<3.0.3: '>=3.0.3'

importers:

  .:
    devDependencies:
      '@next/eslint-plugin-next':
        specifier: 'catalog:'
        version: 16.3.6(eslint@10.11.0(jiti@2.7.0)(supports-color@7.2.0))

  apps/iam-console:
    dependencies:
      '@connectrpc/connect':
        specifier: 'catalog:'
        version: 2.2.0(@bufbuild/protobuf@2.15.0)

snapshots:

  '@bufbuild/protobuf@2.15.0': {}

  '@connectrpc/connect@2.2.0(@bufbuild/protobuf@2.15.0)':
    dependencies:
      '@bufbuild/protobuf': 2.15.0

  '@next/eslint-plugin-next@16.3.6(eslint@10.11.0(jiti@2.7.0)(supports-color@7.2.0))':
    dependencies:
      fast-glob: 3.3.1
    transitivePeerDependencies:
      - eslint

  braces@3.0.3:
    dependencies:
      fill-range: 7.1.1

  fast-glob@3.3.1:
    dependencies:
      micromatch: 4.0.8

  fill-range@7.1.1: {}

  micromatch@4.0.8:
    dependencies:
      braces: 3.0.3
      picomatch: 2.3.2

  picomatch@2.3.2: {}
```

`ci/osv/fixtures/file-dep.yaml`:

```yaml
# SPDX-License-Identifier: Apache-2.0
# expect: 3 shipped-path
# The path to braces runs through a `file:` dependency, in the shape of the real
# '@paigasus/node-bindings' entry of packages/paigasus-kernel.
lockfileVersion: '9.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

importers:

  .:
    devDependencies:
      typescript:
        specifier: 'catalog:'
        version: 6.0.3

  apps/iam-console:
    dependencies:
      '@paigasus/kernel':
        specifier: workspace:*
        version: link:../../packages/paigasus-kernel

  packages/paigasus-kernel:
    dependencies:
      '@paigasus/node-bindings':
        specifier: file:../../../rs/crates/bindings/paigasus-node-bindings
        version: file:../rs/crates/bindings/paigasus-node-bindings

snapshots:

  '@paigasus/node-bindings@file:../rs/crates/bindings/paigasus-node-bindings':
    dependencies:
      '@next/eslint-plugin-next': 16.3.6(eslint@10.11.0(jiti@2.7.0)(supports-color@7.2.0))

  '@next/eslint-plugin-next@16.3.6(eslint@10.11.0(jiti@2.7.0)(supports-color@7.2.0))':
    dependencies:
      fast-glob: 3.3.1
    transitivePeerDependencies:
      - eslint

  braces@3.0.3:
    dependencies:
      fill-range: 7.1.1

  fast-glob@3.3.1:
    dependencies:
      micromatch: 4.0.8

  fill-range@7.1.1: {}

  micromatch@4.0.8:
    dependencies:
      braces: 3.0.3
      picomatch: 2.3.2

  picomatch@2.3.2: {}

  typescript@6.0.3: {}
```

`ci/osv/fixtures/bad-version.yaml`:

```yaml
# SPDX-License-Identifier: Apache-2.0
# expect: 2 lockfile-version
# A pnpm 6.0 lock file. The parser knows only format 9.0, so the guard must refuse it.
lockfileVersion: '6.0'

importers:

  .:
    devDependencies:
      '@next/eslint-plugin-next':
        specifier: 'catalog:'
        version: 16.3.6(eslint@10.11.0(jiti@2.7.0)(supports-color@7.2.0))

  apps/iam-console:
    dependencies:
      micromatch:
        specifier: 'catalog:'
        version: 4.0.8

snapshots:

  '@next/eslint-plugin-next@16.3.6(eslint@10.11.0(jiti@2.7.0)(supports-color@7.2.0))':
    dependencies:
      fast-glob: 3.3.1

  braces@3.0.3: {}

  fast-glob@3.3.1:
    dependencies:
      micromatch: 4.0.8

  micromatch@4.0.8:
    dependencies:
      braces: 3.0.3
```

`ci/osv/fixtures/dangling.yaml`:

```yaml
# SPDX-License-Identifier: Apache-2.0
# expect: 2 dangling-edge
# A snapshot edge points to '@bufbuild/protobuf@2.15.1', which has no snapshot. Without the
# dangling-edge check the walk would stop there and report a clean result.
lockfileVersion: '9.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

importers:

  .:
    devDependencies:
      '@next/eslint-plugin-next':
        specifier: 'catalog:'
        version: 16.3.6(eslint@10.11.0(jiti@2.7.0)(supports-color@7.2.0))

  apps/iam-console:
    dependencies:
      '@connectrpc/connect':
        specifier: 'catalog:'
        version: 2.2.0(@bufbuild/protobuf@2.15.0)

snapshots:

  '@bufbuild/protobuf@2.15.0': {}

  '@connectrpc/connect@2.2.0(@bufbuild/protobuf@2.15.0)':
    dependencies:
      '@bufbuild/protobuf': 2.15.1

  '@next/eslint-plugin-next@16.3.6(eslint@10.11.0(jiti@2.7.0)(supports-color@7.2.0))':
    dependencies:
      fast-glob: 3.3.1
    transitivePeerDependencies:
      - eslint

  braces@3.0.3:
    dependencies:
      fill-range: 7.1.1

  fast-glob@3.3.1:
    dependencies:
      micromatch: 4.0.8

  fill-range@7.1.1: {}

  micromatch@4.0.8:
    dependencies:
      braces: 3.0.3
      picomatch: 2.3.2

  picomatch@2.3.2: {}
```

`ci/osv/fixtures/no-edges.yaml`:

```yaml
# SPDX-License-Identifier: Apache-2.0
# expect: 2 edge-control
# braces is a direct root dependency of `.` only. No snapshot has an edge to it, so the walk
# does not prove that it follows snapshot edges, and the guard must refuse to pass.
lockfileVersion: '9.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

importers:

  .:
    devDependencies:
      '@next/eslint-plugin-next':
        specifier: 'catalog:'
        version: 16.3.6(eslint@10.11.0(jiti@2.7.0)(supports-color@7.2.0))
      braces:
        specifier: ^3.0.3
        version: 3.0.3

  apps/iam-console:
    dependencies:
      '@connectrpc/connect':
        specifier: 'catalog:'
        version: 2.2.0(@bufbuild/protobuf@2.15.0)

snapshots:

  '@bufbuild/protobuf@2.15.0': {}

  '@connectrpc/connect@2.2.0(@bufbuild/protobuf@2.15.0)':
    dependencies:
      '@bufbuild/protobuf': 2.15.0

  '@next/eslint-plugin-next@16.3.6(eslint@10.11.0(jiti@2.7.0)(supports-color@7.2.0))':
    dependencies:
      fast-glob: 3.3.1
    transitivePeerDependencies:
      - eslint

  braces@3.0.3:
    dependencies:
      fill-range: 7.1.1

  fast-glob@3.3.1:
    dependencies:
      picomatch: 2.3.2

  fill-range@7.1.1: {}

  picomatch@2.3.2: {}
```

`ci/osv/fixtures/table-stale.yaml`:

```yaml
# SPDX-License-Identifier: Apache-2.0
# expect: 2 table-stale
# config: table-stale.toml
# The lock file alone would pass. The config names GHSA-vfj7-8cjw-p6xm only in a comment, so
# SHIPPED_FREE_WAIVERS names a waiver that does not exist.
lockfileVersion: '9.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

importers:

  .:
    devDependencies:
      '@next/eslint-plugin-next':
        specifier: 'catalog:'
        version: 16.3.6(eslint@10.11.0(jiti@2.7.0)(supports-color@7.2.0))

  apps/iam-console:
    dependencies:
      '@connectrpc/connect':
        specifier: 'catalog:'
        version: 2.2.0(@bufbuild/protobuf@2.15.0)

snapshots:

  '@bufbuild/protobuf@2.15.0': {}

  '@connectrpc/connect@2.2.0(@bufbuild/protobuf@2.15.0)':
    dependencies:
      '@bufbuild/protobuf': 2.15.0

  '@next/eslint-plugin-next@16.3.6(eslint@10.11.0(jiti@2.7.0)(supports-color@7.2.0))':
    dependencies:
      fast-glob: 3.3.1
    transitivePeerDependencies:
      - eslint

  braces@3.0.3:
    dependencies:
      fill-range: 7.1.1

  fast-glob@3.3.1:
    dependencies:
      micromatch: 4.0.8

  fill-range@7.1.1: {}

  micromatch@4.0.8:
    dependencies:
      braces: 3.0.3
      picomatch: 2.3.2

  picomatch@2.3.2: {}
```

- [ ] **Step 2: Write the guard with a stub for the checks**

Write `ci/osv/shipped_reachability.py` with this content. The parser, the walk and the self-test harness are complete. Only `_check` is a stub, so that the self-test shows red first.

```python
# SPDX-License-Identifier: Apache-2.0
"""Keep the 'no shipped package reaches it' reason of an osv waiver true (SMA-733).

osv-scanner applies an [[IgnoredVulns]] entry to an advisory ID for EVERY path in the lock
file. Some waivers in osv-scanner.toml are justified only because no shipped package reaches
the vulnerable package. This guard walks ts/pnpm-lock.yaml and fails when a shipped importer
reaches a package that SHIPPED_FREE_WAIVERS names. It uses only the Python standard library,
because the `osv advisory scan` job has no uv, node or pnpm (spec F10).

A shipped importer is an importer whose key starts with `apps/`, plus every importer that such
an importer reaches through a `link:` entry. Its roots are its dependencies,
optionalDependencies AND devDependencies: ts/Dockerfile installs dev dependencies, and a
bundler can inline anything that app code imports. The root importer `.` is never shipped.

Exit codes are DELIBERATELY not the repo's usual 0/1/2:
  0  every table entry is reached only from non-shipped importers
  2  infrastructure: the inputs are wrong or the walk cannot be trusted (reason token in the
     first output line)
  3  a shipped importer reaches a waived package (token `shipped-path`)
An uncaught traceback exits 1. ci/osv/run.sh maps 3 -> finding and EVERY other code -> 2, so
a crash can never read as a finding or as a pass.

usage: shipped_reachability.py [--lockfile PATH] [--config PATH] | --self-test
Spec: docs/superpowers/specs/2026-10-04-sma-733-braces-osv-waiver-design.md
"""

from __future__ import annotations

import argparse
import collections
import posixpath
import re
import sys
from pathlib import Path

RC_OK = 0
RC_INFRA = 2
RC_SHIPPED = 3

# Each osv waiver whose reason says "no shipped package reaches it", mapped to the npm package
# it waives. osv-scanner.toml's header requires an entry here for every such waiver.
SHIPPED_FREE_WAIVERS = {"GHSA-vfj7-8cjw-p6xm": "braces"}

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
FIXTURES = HERE / "fixtures"
DEFAULT_FIXTURE_CONFIG = "waiver-config.toml"

IMPORTER_KINDS = ("dependencies", "optionalDependencies", "devDependencies")
SNAPSHOT_EDGE_KINDS = ("dependencies", "optionalDependencies")

# The exact fixture set --self-test must find. Deleting or adding a fixture reds the self-test
# until this tuple changes, so a lost fixture cannot make the self-test pass vacuously.
EXPECTED_FIXTURES = (
    "bad-version.yaml",
    "block-end.yaml",
    "catalogs.yaml",
    "clean.yaml",
    "dangling.yaml",
    "file-dep.yaml",
    "no-edges.yaml",
    "shipped-alias.yaml",
    "shipped-dev.yaml",
    "shipped-link.yaml",
    "shipped-optional.yaml",
    "shipped-prod.yaml",
    "table-stale.yaml",
)


class GuardError(Exception):
    """A check failed. `rc` is the exit code, `token` the reason token."""

    def __init__(self, rc: int, token: str, lines: list[str]) -> None:
        super().__init__(token)
        self.rc = rc
        self.token = token
        self.lines = lines


def infra(token: str, *detail: str) -> GuardError:
    return GuardError(RC_INFRA, token, [f"osv shipped-guard: FAIL {token}: {detail[0]}", *detail[1:]])


def unquote(text: str) -> str:
    """Remove ONE pair of matching ' or " quotes."""
    if len(text) >= 2 and text[0] == text[-1] and text[0] in "'\"":
        return text[1:-1]
    return text


def split_entry(body: str) -> tuple[str, str | None]:
    """Split `key: value` or `key:` (indent already removed). The value is None for `key:`."""
    if body[:1] in "'\"":
        end = body.find(body[0], 1)
        if end > 0 and body[end + 1 : end + 2] == ":":
            rest = body[end + 2 :].strip()
            return body[1:end], (unquote(rest) if rest else None)
    if body.endswith(":"):
        return body[:-1], None
    key, sep, value = body.partition(": ")
    if not sep:
        return body, None
    return unquote(key.strip()), unquote(value.strip())


def parse_lockfile(text: str) -> tuple[str | None, dict, dict]:
    """Return (lockfileVersion, importers, snapshots) from a pnpm 9.0 lock file.

    importers: {importer: {kind: {name: version}}}
    snapshots: {snapshot key: [(name, version), ...]}
    Every other top-level section (settings, catalogs, overrides, packages) is skipped.
    """
    version = None
    importers: dict[str, dict[str, dict[str, str | None]]] = {}
    snapshots: dict[str, list[tuple[str, str | None]]] = {}
    section = None
    importer = kind = dep = snap = None
    in_edges = False
    for raw in text.splitlines():
        body = raw.strip()
        if not body or body.startswith("#"):
            continue
        indent = len(raw) - len(raw.lstrip(" "))
        key, value = split_entry(body)
        if indent == 0:
            section = key
            importer = kind = dep = snap = None
            in_edges = False
            if key == "lockfileVersion":
                version = value
            continue
        if section == "importers":
            if indent == 2:
                importer, kind, dep = key, None, None
                importers[importer] = {k: {} for k in IMPORTER_KINDS}
            elif indent == 4:
                kind, dep = (key if key in IMPORTER_KINDS else None), None
            elif indent == 6 and kind is not None:
                dep = key
                importers[importer][kind][dep] = None
            elif indent == 8 and kind is not None and dep is not None and key == "version":
                importers[importer][kind][dep] = value
        elif section == "snapshots":
            if indent == 2:
                snap, in_edges = key, False
                snapshots[snap] = []
            elif indent == 4:
                # Only a dependencies/optionalDependencies BLOCK gives edges. Any other key at
                # indent 4 (transitivePeerDependencies:, optional: true) ends the edge block.
                in_edges = key in SNAPSHOT_EDGE_KINDS and value is None
            elif indent == 6 and in_edges:
                snapshots[snap].append((key, value))
    return version, importers, snapshots


def node_for(importer: str, name: str, value: str | None) -> tuple[str, str]:
    """Map one `name: version` entry to a node: ('importer', path) or ('snapshot', key)."""
    if value is None:
        return ("snapshot", f"{name}@<no version>")
    if value.startswith("link:"):
        return ("importer", posixpath.normpath(posixpath.join(importer, value[len("link:") :])))
    if value.startswith("file:"):
        return ("snapshot", f"{name}@{value}")
    head = value.split("(", 1)[0]
    if "@" in head[1:]:
        return ("snapshot", value)  # an alias: `name: realname@version`
    return ("snapshot", f"{name}@{value}")


def package_name(snapshot_key: str) -> str:
    head = snapshot_key.split("(", 1)[0]
    at = head.rfind("@")
    return head[:at] if at > 0 else head


def build_graph(importers: dict, snapshots: dict) -> dict:
    """Return {node: [child node, ...]}. Raise `dangling-edge` for any edge to a missing node."""
    graph: dict[tuple[str, str], list[tuple[str, str]]] = {}
    for imp, kinds in importers.items():
        children = []
        for kind in IMPORTER_KINDS:
            for name, value in kinds[kind].items():
                children.append(node_for(imp, name, value))
        graph[("importer", imp)] = children
    for key, edges in snapshots.items():
        graph[("snapshot", key)] = [node_for(".", name, value) for name, value in edges]
    for node, children in graph.items():
        for child in children:
            if child not in graph:
                raise infra(
                    "dangling-edge",
                    f"{label(node)} has an edge to {child[0]} '{child[1]}', which the lock file does not define",
                    "  The parser and the lock file disagree; a walk over it cannot be trusted.",
                )
    return graph


def label(node: tuple[str, str]) -> str:
    return node[1]


def walk(graph: dict, starts: list) -> dict:
    """Breadth-first walk. Return {reached node: parent node or None}."""
    parent: dict[tuple[str, str], tuple[str, str] | None] = {}
    queue = collections.deque()
    for start in starts:
        if start not in parent:
            parent[start] = None
            queue.append(start)
    while queue:
        node = queue.popleft()
        # .get: build_graph already refused a dangling edge. If that check is ever lost, the
        # walk stops at the missing node, and dangling.yaml reds as a plain mismatch.
        for child in graph.get(node, []):
            if child not in parent:
                parent[child] = node
                queue.append(child)
    return parent


def path_to(parent: dict, node: tuple[str, str]) -> list[str]:
    out = []
    cur = node
    while cur is not None:
        out.append(label(cur))
        cur = parent[cur]
    return list(reversed(out))


def check(lockfile: Path, config: Path) -> tuple[int, list[str]]:
    """Run every check. Return (exit code, output lines). The first line holds the token."""
    try:
        return RC_OK, _check(lockfile, config)
    except GuardError as exc:
        return exc.rc, exc.lines


def _check(lockfile: Path, config: Path) -> list[str]:
    raise infra("not-implemented", "Task 1 Step 4 replaces this stub with the real checks")


def fixture_expectation(path: Path) -> tuple[int, str | None, Path]:
    """Read `# expect: <rc> [token]` and the optional `# config: <file>` header lines."""
    expect = None
    config = FIXTURES / DEFAULT_FIXTURE_CONFIG
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith("# expect:"):
            expect = line[len("# expect:") :].split()
        elif line.startswith("# config:"):
            config = FIXTURES / line[len("# config:") :].strip()
    if not expect:
        raise SystemExit(f"osv shipped-guard self-test: {path.name} has no '# expect:' line")
    return int(expect[0]), (expect[1] if len(expect) > 1 else None), config


def self_test() -> int:
    found = tuple(sorted(p.name for p in FIXTURES.glob("*.yaml")))
    failures = []
    if found != EXPECTED_FIXTURES:
        failures.append(f"fixture set differs: found {list(found)}, expected {list(EXPECTED_FIXTURES)}")
    for name in found:
        want_rc, want_token, config = fixture_expectation(FIXTURES / name)
        try:
            got_rc, lines = check(FIXTURES / name, config)
        except Exception as exc:  # name the fixture; never let one crash hide the others
            got_rc, lines = 1, [f"{type(exc).__name__}: {exc}"]
        first = lines[0] if lines else ""
        token_ok = want_token is None or f"FAIL {want_token}:" in first
        if got_rc != want_rc or not token_ok:
            failures.append(f"{name}: expected rc {want_rc} {want_token or ''}, got rc {got_rc}: {first}")
    if failures:
        for failure in failures:
            print(f"  FAIL {failure}", file=sys.stderr)
        print(f"osv shipped-guard self-test: {len(failures)} failure(s)", file=sys.stderr)
        return RC_INFRA
    print(f"osv shipped-guard self-test: {len(found)} fixtures passed")
    return RC_OK


def main(argv: list[str]) -> int:
    if sys.version_info < (3, 9):  # noqa: UP036 - the scan job's python3 is not pinned (spec F10)
        print("osv shipped-guard: FAIL python-too-old: Python 3.9 or later is required", file=sys.stderr)
        return RC_INFRA
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--lockfile", default=str(REPO / "ts" / "pnpm-lock.yaml"))
    parser.add_argument("--config", default=str(REPO / "osv-scanner.toml"))
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv[1:])
    if args.self_test:
        return self_test()
    rc, lines = check(Path(args.lockfile), Path(args.config))
    for line in lines:
        print(line, file=sys.stdout if rc == RC_OK else sys.stderr)
    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv))
```

- [ ] **Step 3: Run the self-test and see it fail**

Run: `python3 ci/osv/shipped_reachability.py --self-test; echo "rc=$?"`

Expected: 13 lines of the form `  FAIL <fixture>: expected rc ..., got rc 2: osv shipped-guard: FAIL not-implemented: ...`, then `osv shipped-guard self-test: 13 failure(s)` and `rc=2`. If a fixture does not show in the list, the fixture file or its `# expect:` line is wrong. Fix the fixture before you continue.

- [ ] **Step 4: Replace the stub with the real checks**

In `ci/osv/shipped_reachability.py`, replace this text:

```python
def _check(lockfile: Path, config: Path) -> list[str]:
    raise infra("not-implemented", "Task 1 Step 4 replaces this stub with the real checks")
```

with this text:

```python
def _check(lockfile: Path, config: Path) -> list[str]:
    if not lockfile.is_file():
        raise infra("no-lockfile", f"'{lockfile}' does not exist")
    if not config.is_file():
        raise infra("no-config", f"'{config}' does not exist")
    version, importers, snapshots = parse_lockfile(lockfile.read_text(encoding="utf-8"))
    if version != "9.0":
        raise infra("lockfile-version", f"lockfileVersion is {version!r}, not '9.0'; update the parser in {Path(__file__).name}")
    if not importers or not snapshots:
        raise infra("empty-section", f"parsed {len(importers)} importers and {len(snapshots)} snapshots; both must be non-zero")
    apps = sorted(i for i in importers if i.startswith("apps/"))
    if not apps:
        raise infra("no-shipped-importer", "no importer key starts with 'apps/'")
    graph = build_graph(importers, snapshots)
    config_text = config.read_text(encoding="utf-8")
    for ghsa in SHIPPED_FREE_WAIVERS:
        if not re.search(r'^[ \t]*id[ \t]*=[ \t]*"' + re.escape(ghsa) + r'"[ \t]*$', config_text, re.MULTILINE):
            raise infra(
                "table-stale",
                f"SHIPPED_FREE_WAIVERS names {ghsa}, but '{config}' has no `id = \"{ghsa}\"` line",
                f"  Remove {ghsa} from SHIPPED_FREE_WAIVERS in {Path(__file__).name}, or restore the waiver.",
            )

    shipped = walk(graph, [("importer", a) for a in apps])
    control = walk(graph, [("importer", i) for i in sorted(importers)])

    for ghsa, package in SHIPPED_FREE_WAIVERS.items():
        # Edge control: some snapshot the control walk reached must have an edge to the package,
        # so the package is reached through a path of at least two edges. A direct root alone
        # does not prove that the walk follows snapshot edges, quoted keys and peer suffixes.
        via = sorted(
            node
            for node in control
            if node[0] == "snapshot" and any(c[0] == "snapshot" and package_name(c[1]) == package for c in graph.get(node, []))
        )
        if not via:
            raise infra(
                "edge-control",
                f"the control walk reaches no snapshot with an edge to '{package}' ({ghsa})",
                "  Either the walk no longer follows snapshot edges (a parser regression), or the package",
                f"  left the lock file. In the second case the waiver is stale: remove {ghsa} from",
                "  osv-scanner.toml and from SHIPPED_FREE_WAIVERS.",
            )

    for ghsa, package in SHIPPED_FREE_WAIVERS.items():
        hits = sorted(n for n in shipped if n[0] == "snapshot" and package_name(n[1]) == package)
        if hits:
            lines = [f"osv shipped-guard: FAIL shipped-path: a shipped importer reaches '{package}' ({ghsa})"]
            lines += ["  " + " > ".join(path_to(shipped, hit)) for hit in hits]
            lines += [
                f"  The waiver for {ghsa} in osv-scanner.toml says no shipped package reaches '{package}'.",
                "  Remove the waiver and fix the path, or rewrite the waiver's reason.",
            ]
            raise GuardError(RC_SHIPPED, "shipped-path", lines)

    return [
        f"osv shipped-guard: {package} ({ghsa}) reached only from dev tooling, not from a shipped importer"
        for ghsa, package in SHIPPED_FREE_WAIVERS.items()
    ]
```

- [ ] **Step 5: Run the self-test under both pythons**

Run: `python3 ci/osv/shipped_reachability.py --self-test; echo "rc=$?"`
Run: `/usr/bin/python3 ci/osv/shipped_reachability.py --self-test; echo "rc=$?"`

Expected for each: `osv shipped-guard self-test: 13 fixtures passed` and `rc=0`. `/usr/bin/python3` must be 3.9.6 (`/usr/bin/python3 --version`).

- [ ] **Step 6: Run the guard on the real lock file**

Run: `python3 ci/osv/shipped_reachability.py; echo "rc=$?"`

Expected: `osv shipped-guard: FAIL table-stale: SHIPPED_FREE_WAIVERS names GHSA-vfj7-8cjw-p6xm, but '<repo>/osv-scanner.toml' has no ...` and `rc=2`. This is correct at this point: Task 2 adds the waiver. The result also proves that the parser read the real lock file without a `lockfile-version`, `empty-section`, `no-shipped-importer` or `dangling-edge` refusal.

- [ ] **Step 7: Run the self-test from a different working directory (Review Focus 5)**

Run: `cd /tmp && python3 "$OLDPWD/ci/osv/shipped_reachability.py" --self-test; echo "rc=$?"; cd "$OLDPWD"`

Expected: `13 fixtures passed` and `rc=0`.

- [ ] **Step 8: Lint the guard**

Run: `uv run --locked --project py ruff check --config py/pyproject.toml ci/osv/shipped_reachability.py; echo "rc=$?"`

Expected: `All checks passed!` and `rc=0`. Do not remove the `# noqa: UP036` comment: without it, ruff reports UP036 (measured).

- [ ] **Step 9: Commit**

```bash
git add ci/osv/shipped_reachability.py ci/osv/fixtures
git commit -m "$(cat <<'EOF'
feat(ci): add the osv shipped-path guard and its fixtures (SMA-733)

The guard walks ts/pnpm-lock.yaml with the standard library only. It
fails when a shipped importer reaches a package that a waiver says no
shipped package reaches. It exits 3 for a finding and 2 for an
infrastructure refusal, so a traceback (1) cannot read as either.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

### Task 2: Wire the guard into `run.sh`, add the waiver, update the documents

**Files:**
- Modify: `ci/osv/run.sh` (the header after line 28, the area after the package-count loop at line 86, and the findings block at lines 88-126)
- Modify: `osv-scanner.toml` (whole file, 19 lines now)
- Modify: `ci/CLAUDE.md:59-60`

**Interfaces:**
- Consumes: from Task 1, the command line `python3 ci/osv/shipped_reachability.py --self-test` and `python3 ci/osv/shipped_reachability.py --lockfile ts/pnpm-lock.yaml --config osv-scanner.toml`, with the exit codes 0, 2, 3 and 1 (traceback).
- Produces: `ci/osv/run.sh` exits 0, 1 or 2 per the Global Constraints, and always prints the line `osv gate: summary: osv-scanner: <...>; shipped-path guard: <...>.` before it exits (after the package counts). Task 3 mutates the guard call line.

- [ ] **Step 1: Run the gate and see it fail**

Run: `ci/osv/run.sh; echo "rc=$?"`

Expected: `osv gate: vulnerabilities found.`, a block that names `braces@3.0.3` and `GHSA-vfj7-8cjw-p6xm`, and `rc=1`. This is the failure that SMA-733 removes.

- [ ] **Step 2: Add the guard section to the `run.sh` header**

In `ci/osv/run.sh`, replace this text:

```bash
# must never degrade to a silent pass.
set -uo pipefail
```

with this text:

```bash
# must never degrade to a silent pass.
#
# SHIPPED-PATH GUARD (SMA-733). Some waivers in osv-scanner.toml are true only while no
# shipped package reaches the waived package. ci/osv/shipped_reachability.py walks
# ts/pnpm-lock.yaml and keeps that true for every advisory in its SHIPPED_FREE_WAIVERS
# table. This script runs it twice, `--self-test` first and then on the real files, and maps
# each exit code:
#
#   0          pass
#   3          the real run found a shipped path to a waived package (a finding, guard=1)
#   any other  infrastructure error (guard=2): 1 is a traceback, 2 is a guard refusal, 127
#              is a missing python3. A self-test that does not exit 0 is always guard=2.
#
# The findings block runs also when the guard failed, so a new, unrelated osv finding still
# prints. Exit precedence: 2 if the guard is 2 (or osv-scanner itself failed, above); else 1
# if osv found something or the guard is 1; else 0.
# Residual: nothing reds if someone deletes the guard calls below (SMA-733 spec section 4.5).
set -uo pipefail
```

- [ ] **Step 3: Add the guard calls and make the findings block not exit**

In `ci/osv/run.sh`, replace this text:

```bash
# --- Findings -----------------------------------------------------------------------------
if [ "$rc" -eq 1 ]; then
  echo "" >&2
```

with this text:

```bash
# --- Shipped-path guard (SMA-733) ---------------------------------------------------------
# No `-e` here, so each status is captured with `|| var=$?` and mapped explicitly. An
# unmapped status must never read as clean (see the header).
guard=0
selftest_rc=0
python3 ci/osv/shipped_reachability.py --self-test || selftest_rc=$?
guard_rc=0
python3 ci/osv/shipped_reachability.py --lockfile ts/pnpm-lock.yaml --config osv-scanner.toml || guard_rc=$?
if [ "$selftest_rc" -ne 0 ]; then
  echo "osv gate: the shipped-path guard's self-test exited $selftest_rc — the guard cannot be trusted." >&2
  guard=2
fi
case "$guard_rc" in
  0) ;;
  3) [ "$guard" -eq 2 ] || guard=1 ;;
  *)
    echo "osv gate: the shipped-path guard exited $guard_rc (1=traceback, 2=refusal, 127=no python3) — infrastructure error." >&2
    guard=2
    ;;
esac

# --- Findings -----------------------------------------------------------------------------
osv_found=0
if [ "$rc" -eq 1 ]; then
  osv_found=1
  echo "" >&2
```

- [ ] **Step 4: Replace the early `exit 1` with the verdict block**

In `ci/osv/run.sh`, replace this text:

```bash
  echo "  justified [[IgnoredVulns]] entry in osv-scanner.toml." >&2
  exit 1
fi

echo "osv gate: no known vulnerabilities in the npm or pip lockfiles."
```

with this text:

```bash
  echo "  justified [[IgnoredVulns]] entry in osv-scanner.toml." >&2
fi

# --- Verdict --------------------------------------------------------------------------------
case "$guard" in 0) guard_word='pass' ;; 1) guard_word='shipped path found' ;; *) guard_word='infrastructure error' ;; esac
if [ "$osv_found" -eq 1 ]; then osv_word='vulnerabilities found'; else osv_word='no known vulnerabilities'; fi
echo "osv gate: summary: osv-scanner: $osv_word; shipped-path guard: $guard_word."
if [ "$guard" -eq 2 ]; then
  exit 2
fi
if [ "$osv_found" -eq 1 ] || [ "$guard" -eq 1 ]; then
  exit 1
fi
echo "osv gate: no known vulnerabilities in the npm or pip lockfiles."
```

- [ ] **Step 5: Check the gate with the guard but without the waiver**

Run: `ci/osv/run.sh; echo "rc=$?"`

Expected, in this order: the four package-count lines; `osv shipped-guard self-test: 13 fixtures passed`; `osv shipped-guard: FAIL table-stale: ...`; `osv gate: the shipped-path guard exited 2 ...`; `osv gate: vulnerabilities found.` with `braces@3.0.3` and `GHSA-vfj7-8cjw-p6xm`; `osv gate: summary: osv-scanner: vulnerabilities found; shipped-path guard: infrastructure error.`; `rc=2`. This is spec section 5 step 3: both results print, and the guard's 2 wins.

- [ ] **Step 6: Write the new `osv-scanner.toml`**

Replace the whole file with this content:

```toml
# osv-scanner waivers for the npm + pip lockfiles (SMA-518).
#
# This is the JS/Python counterpart to the `[advisories] ignore` list in rs/deny.toml, and
# it is held to the same standard: an entry must explain why the vulnerable code path is
# unreachable in this repo, or why no upgrade exists — not merely that the finding is
# inconvenient. `rs/deny.toml`'s RUSTSEC-2023-0071 entry is the model. An unjustified
# waiver is worse than the alert backlog it replaces, because it is invisible.
#
# Prefer fixing over waiving. For a pinned npm transitive, add a range-scoped selector to
# the `overrides:` block in ts/pnpm-workspace.yaml; for pip, `uv lock --upgrade-package`.
#
# Format (osv-scanner 2.x):
#
#   [[IgnoredVulns]]
#   id = "GHSA-xxxx-xxxx-xxxx"
#   # ignoreUntil = 2026-01-01   # optional — the waiver re-fires after this date
#   reason = "Why the vulnerable path is unreachable here, or why no fix exists."
#
# When a waiver's reason depends on "no shipped package reaches it", the advisory must also
# be in SHIPPED_FREE_WAIVERS in ci/osv/shipped_reachability.py. ci/osv/run.sh runs that
# guard, and it fails when a shipped importer reaches the waived package.
#
# The first waiver is GHSA-vfj7-8cjw-p6xm (braces, SMA-733). Every earlier finding was
# fixed by upgrading.

# GHSA-vfj7-8cjw-p6xm (CVE-2026-93687, severity 8.7): "braces vulnerable to
# stack-exhaustion denial of service through deeply nested patterns". It affects
# braces@3.0.3 in ts/pnpm-lock.yaml.
#
# No fix: osv.dev records introduced=0, last_affected=3.0.3, and 3.0.3 is the newest braces
# on npm. No upgrade removes the path: the newest micromatch (4.0.8), fast-glob (3.3.3),
# @next/eslint-plugin-next (16.3.8, which pins fast-glob 3.3.1), semantic-release (25.0.9)
# and @semantic-release/commit-analyzer (13.0.1) all still reach braces.
#
# Who reaches it: only micromatch@4.0.8, and only from the root importer `.` through
# devDependencies (@next/eslint-plugin-next -> fast-glob, semantic-release,
# @semantic-release/commit-analyzer). No console image contains it: ts/Dockerfile installs
# with --filter "@paigasus/${APP}...", which excludes the root importer, and the image ships
# only .next/standalone and .next/static.
#
# Threat model: micromatch calls braces only to expand a glob pattern, and fast-glob expands
# only the patterns it gets. These tools expand only patterns that the repo's own config
# holds (for example the ESLint config's settings.next.rootDir). A fork PR can change those
# patterns. The worst result is then a stack exhaustion that stops that PR's own CI job,
# which has no write credentials. No workflow runs semantic-release, so no release job runs
# braces on untrusted input.
#
# Guard: ci/osv/shipped_reachability.py fails `repo:osv` when a shipped importer (apps/* and
# the workspace packages they link) reaches braces. Residual: nothing reds if someone
# deletes the guard call from ci/osv/run.sh (SMA-733 spec section 4.5).
[[IgnoredVulns]]
id = "GHSA-vfj7-8cjw-p6xm"
ignoreUntil = 2027-01-02
reason = "No fixed braces release. Only root dev tooling (ESLint Next plugin, semantic-release) reaches it, on repo-authored glob patterns; no shipped package does, and ci/osv/shipped_reachability.py enforces that (SMA-733)."
```

- [ ] **Step 7: Update the osv policy line in `ci/CLAUDE.md`**

In `ci/CLAUDE.md`, replace this text:

```markdown
  `overrides:` selector or `uv lock --upgrade-package` — or a justified `osv-scanner.toml`
  waiver; a dep consumed only by a later commit needs a temporary
```

with this text:

```markdown
  `overrides:` selector or `uv lock --upgrade-package` — or a justified `osv-scanner.toml`
  waiver (a waiver whose reason depends on "no shipped package reaches it" also needs a
  `SHIPPED_FREE_WAIVERS` entry in `ci/osv/shipped_reachability.py`, SMA-733); a dep consumed
  only by a later commit needs a temporary
```

- [ ] **Step 8: Run the gate under both bashes**

Run: `/bin/bash ci/osv/run.sh; echo "rc=$?"`
Run: `/opt/homebrew/bin/bash ci/osv/run.sh; echo "rc=$?"`

Expected for each, in this order: `osv gate: ts/pnpm-lock.yaml 1128 packages scanned` (the number can differ if the lock file changed) and the three pip lines; `osv shipped-guard self-test: 13 fixtures passed`; `osv shipped-guard: braces (GHSA-vfj7-8cjw-p6xm) reached only from dev tooling, not from a shipped importer`; `osv gate: summary: osv-scanner: no known vulnerabilities; shipped-path guard: pass.`; `osv gate: no known vulnerabilities in the npm or pip lockfiles.`; `rc=0`.

- [ ] **Step 9: Run the gate through Moon**

Run: `moon run repo:osv --force; echo "rc=$?"`

Expected: the task passes and `rc=0`.

- [ ] **Step 10: Commit**

```bash
git add ci/osv/run.sh osv-scanner.toml ci/CLAUDE.md
git commit -m "$(cat <<'EOF'
fix(ci): waive the braces advisory behind the shipped-path guard (SMA-733)

GHSA-vfj7-8cjw-p6xm has no fixed braces release, and no upgrade removes
the path. Only root dev tooling reaches braces. The waiver expires on
2027-01-02. run.sh now runs the guard, maps every exit code other than
0 and 3 to an infrastructure error, and always prints the osv findings.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 11: Check that the merge into PR 363 stays clean**

PR 363 (branch `feature/sma-693-wasm-bindgen-lockstep-updater`) changes only the `LOCKFILES` array of `ci/osv/run.sh` (spec F15). This task changes other lines of that file, so the merge must be clean.

Run: `git fetch origin feature/sma-693-wasm-bindgen-lockstep-updater`
Run: `git merge-tree --write-tree HEAD FETCH_HEAD >/dev/null; echo "rc=$?"`

Expected: `rc=0` (no conflict). `git merge-tree --write-tree` writes no file in the worktree (git 2.38 or later; the development Mac has 2.54.0). If the result is not 0, stop and report the conflicting paths.

### Task 3: Verification: mutations, both pythons, build output, artifact list, full graph

**Files:**
- No repo file changes. Each mutation is temporary, and each step ends with `git status --porcelain` empty.
- Scratch output only (for the PR body): `$SCRATCH/sma-733-evidence.txt`, where `SCRATCH` is a scratch directory outside the repo.

**Interfaces:**
- Consumes: Task 1's guard and its mutation anchor lines; Task 2's `run.sh` guard call line.
- Produces: the evidence for the PR body (the mutation table, the build-output grep, the artifact list).

Mutation procedure, for every mutation below: (1) make the edit exactly as shown, with the `# MUTATION-Mn` marker at the END of the line; (2) run the command; (3) record the result; (4) make the reverse edit (replace the new text with the old text); (5) run `git status --porcelain` and `grep -rn 'MUTATION-' ci/osv`. Both must print nothing. Do not use `git checkout --` or `git stash`.

- [ ] **Step 1: M1a, the guard call points at a shipped fixture**

In `ci/osv/run.sh`, replace this text:

```bash
python3 ci/osv/shipped_reachability.py --lockfile ts/pnpm-lock.yaml --config osv-scanner.toml || guard_rc=$?
```

with this text:

```bash
python3 ci/osv/shipped_reachability.py --lockfile ci/osv/fixtures/shipped-prod.yaml --config osv-scanner.toml || guard_rc=$?  # MUTATION-M1
```

Run: `ci/osv/run.sh; echo "rc=$?"`
Expected: `osv shipped-guard: FAIL shipped-path: ...`, a path line `apps/iam-console > @semantic-release/commit-analyzer@13.0.1(...) > micromatch@4.0.8 > braces@3.0.3`, `shipped-path guard: shipped path found.` and `rc=1`. Restore.

- [ ] **Step 2: M1b, the guard calls are deleted (the residual)**

Delete the two lines that contain `shipped_reachability.py` in `ci/osv/run.sh` (the `--self-test` call and the real call). Do not delete the comment lines in the header.

Run: `ci/osv/run.sh; echo "rc=$?"`
Expected: `rc=0`. The summary line says `shipped-path guard: pass.` although the guard did not run. This is the residual of spec section 4.5: record it in the PR body. Restore the two lines exactly.

- [ ] **Step 3: M2, no `devDependencies`**

In `ci/osv/shipped_reachability.py`, replace this text:

```python
        for kind in IMPORTER_KINDS:
```

with this text:

```python
        for kind in IMPORTER_KINDS[:2]:  # MUTATION-M2
```

Run: `python3 ci/osv/shipped_reachability.py --self-test; echo "rc=$?"`
Expected: `FAIL shipped-dev.yaml: expected rc 3 shipped-path, got rc 2: ... edge-control ...`, also `FAIL block-end.yaml` and `FAIL catalogs.yaml` (both `edge-control`), `3 failure(s)`, `rc=2`. Restore.

- [ ] **Step 4: M3, no snapshot edges**

In `ci/osv/shipped_reachability.py`, replace this text:

```python
                snapshots[snap].append((key, value))
```

with this text:

```python
                pass  # MUTATION-M3
```

Run: `python3 ci/osv/shipped_reachability.py; echo "rc=$?"`
Expected: `osv shipped-guard: FAIL edge-control: ...` and `rc=2`. (The self-test also fails, with 10 failures.) Restore.

- [ ] **Step 5: M4, no dangling-edge check**

In `ci/osv/shipped_reachability.py`, replace this text:

```python
            if child not in graph:
```

with this text:

```python
            if False:  # MUTATION-M4
```

Run: `python3 ci/osv/shipped_reachability.py --self-test; echo "rc=$?"`
Expected: `FAIL dangling.yaml: expected rc 2 dangling-edge, got rc 0: ...`, `1 failure(s)`, `rc=2`. Restore.

- [ ] **Step 6: M5, no `optionalDependencies` blocks**

In `ci/osv/shipped_reachability.py`, replace this text:

```python
SNAPSHOT_EDGE_KINDS = ("dependencies", "optionalDependencies")
```

with this text:

```python
SNAPSHOT_EDGE_KINDS = ("dependencies",)  # MUTATION-M5
```

Run: `python3 ci/osv/shipped_reachability.py --self-test; echo "rc=$?"`
Expected: `FAIL shipped-optional.yaml: expected rc 3 shipped-path, got rc 2: ... edge-control ...`, `1 failure(s)`, `rc=2`.
Run: `ci/osv/run.sh; echo "rc=$?"`
Expected: `osv gate: the shipped-path guard's self-test exited 2 ...`, `shipped-path guard: infrastructure error.` and `rc=2`. The real guard run passes under M5, so only the self-test catches it. Restore.

- [ ] **Step 7: M6, an alias value read as `name@value`**

In `ci/osv/shipped_reachability.py`, replace this text:

```python
    if "@" in head[1:]:
```

with this text:

```python
    if False:  # MUTATION-M6
```

Run: `python3 ci/osv/shipped_reachability.py --self-test; echo "rc=$?"`
Expected: `FAIL shipped-alias.yaml: expected rc 3 shipped-path, got rc 2: ... dangling-edge ... 'micromatch-cjs@micromatch@4.0.8' ...`, `1 failure(s)`, `rc=2`. Restore.

- [ ] **Step 8: M7, no `python3` on `PATH`**

Make a directory that holds only the tools that `run.sh` needs, without `python3`:

```bash
NP="$(mktemp -d)"
ln -s "$(proto bin osv-scanner --reporter text | tail -n1)" "$NP/osv-scanner"
for t in git mktemp grep tail sed cat rm dirname env; do ln -s "$(command -v "$t")" "$NP/$t"; done
PATH="$NP" /bin/bash ci/osv/run.sh; echo "rc=$?"
rm -rf "$NP"
```

Expected: two `python3: command not found` lines, `self-test exited 127`, `the shipped-path guard exited 127`, `shipped-path guard: infrastructure error.` and `rc=2`. This step makes no repo change.

- [ ] **Step 9: M8, the guard raises in its real run**

In `ci/osv/shipped_reachability.py`, replace this text:

```python
    rc, lines = check(Path(args.lockfile), Path(args.config))
```

with this text:

```python
    raise RuntimeError("MUTATION-M8")
    rc, lines = check(Path(args.lockfile), Path(args.config))
```

Run: `ci/osv/run.sh; echo "rc=$?"`
Expected: `13 fixtures passed` (the mutation is only in the real-run path, so the self-test passes), a traceback that ends in `RuntimeError: MUTATION-M8`, `the shipped-path guard exited 1 ...`, `shipped-path guard: infrastructure error.` and `rc=2`. Restore.

- [ ] **Step 10: Confirm that every mutation is restored**

Run: `git status --porcelain; grep -rn 'MUTATION-' ci/osv; python3 ci/osv/shipped_reachability.py --self-test; ci/osv/run.sh; echo "rc=$?"`
Expected: no `git status` output, no `grep` output, `13 fixtures passed`, and `rc=0` at the end.

- [ ] **Step 11: Both pythons and both bashes, once more after the mutations**

Run: `/usr/bin/python3 ci/osv/shipped_reachability.py --self-test; echo "rc=$?"` (expect `rc=0`)
Run: `/bin/bash ci/osv/run.sh; echo "rc=$?"` (expect `rc=0`)
Run: `/opt/homebrew/bin/bash ci/osv/run.sh; echo "rc=$?"` (expect `rc=0`)
Run: `grep -nE 'mapfile|declare -A' ci/osv/run.sh; echo "rc=$?"` (expect no match and `rc=1`)

- [ ] **Step 12: Build both console apps and grep the build output for braces (spec F6, section 5 step 8)**

Run: `moon run iam-console-ts:build gateway-console-ts:build; echo "rc=$?"` (expect `rc=0`)

Then run each command and record its output:

```bash
find ts/apps/iam-console/.next/standalone ts/apps/gateway-console/.next/standalone -type d \( -name braces -o -name micromatch -o -name fast-glob -o -name 'braces@*' \)
grep -rlE 'braces@3|node_modules/braces|/braces/lib|node_modules/micromatch|node_modules/fast-glob' ts/apps/iam-console/.next ts/apps/gateway-console/.next
grep -rlw 'braces' ts/apps/iam-console/.next/static ts/apps/gateway-console/.next/static
```

Expected: no output from the three commands. The older `.next` output in the main checkout gave no output (measured while planning). If the third command finds the plain word `braces`, open each file and check whether it is the `braces` module or an unrelated word (for example in a CSS or text parser). Record what you find. Do not hide a hit: a real `braces` module in the output contradicts the waiver, and the waiver must not merge.

- [ ] **Step 13: Make the artifact list for the PR body (spec section 6)**

Run:

```bash
python3 - <<'PY'
import glob
import json

paths = glob.glob("ts/packages/*/package.json") + glob.glob("ts/apps/*/package.json") + glob.glob("rs/crates/bindings/*/package.json")
for path in sorted(paths):
    with open(path) as fh:
        data = json.load(fh)
    print(path, "private", data.get("private"),
          "deps", sorted(data.get("dependencies") or {}),
          "optional", sorted(data.get("optionalDependencies") or {}))
PY
```

Expected (measured while planning): every `ts/apps/*` and `ts/packages/*` manifest has `private` `True`, so npm publishes none of them. `rs/crates/bindings/paigasus-node-bindings/package.json` (`@paigasus/node-bindings`) and `rs/crates/bindings/paigasus-wasm/package.json` (`@paigasus/wasm`) have no `dependencies` and no `optionalDependencies`. Also run `git ls-files 'rs/crates/bindings/*/npm/*/package.json'`. If it lists platform-package manifests, check each one for `dependencies` the same way.

Run: `grep -c braces py/uv.lock` (expect `0`, spec F7).

Write the PR-body artifact list into `$SCRATCH/sma-733-evidence.txt`: the two console images (Step 12 result); `@paigasus/node-bindings`, its platform packages and `@paigasus/wasm` (no npm dependencies); the Python wheels and `py/uv.lock` (no `braces`); the Rust crates and images (no npm content). Add the mutation table from Steps 1-9 and the residual from Step 2.

- [ ] **Step 14: Run the full gate graph**

Read the root `CLAUDE.md` sections "Before you push: the full gate graph" and "This development Mac only" first. Some gates need system bash 3.2 and some need bash 4+. Pick one bash for the `moon ci` run, then run the gates that need the other bash directly, and read those direct results.

Run:

```bash
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :helm-render :moon-diagnosis-exec :test-e2e \
  --base origin/main \
  --include-relations
```

Expected: every task passes, `repo:osv` and `repo:ruff-ci` included. If a task fails, follow the root `CLAUDE.md` procedure "Diagnosing an unattributed `moon ci` failure", Step 0 first, before any re-run.

- [ ] **Step 15: No commit**

Task 3 changes no repo file. Run `git status --porcelain` (expect no output) and `git log --oneline -3` (expect the two commits of Tasks 1 and 2 on top of the spec commits).

After the PR opens (not part of this plan's tasks, see spec section 7): `CI` and `osv advisory scan` must pass on the PR; after the merge, start `security-scan` with `workflow_dispatch` on `main`; merge `main` into PR 363; make the Linear reminder issue due 2026-12-19.
