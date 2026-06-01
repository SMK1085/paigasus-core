# Contributing to paigasus-core

Thanks for your interest in contributing. This document is the canonical guide
to how we work.

## Reporting issues

Open a [GitHub Issue](../../issues). The maintainer triages reports into an
internal Linear tracker, so you don't need Linear access to file one. Where you
can, include reproduction steps and name the affected workspace (`contracts`,
`rs`, `py`, or `ts`).

## Development workflow

1. Branch off `main` as `feature/sma-NNN-<slug>`, where `sma-NNN` is the Linear
   issue key and `<slug>` is a short kebab-case description — e.g.
   `feature/sma-357-bootstrap-rs-cargo-workspace`. External contributors without
   a Linear key may use `feature/<slug>`.
2. Make focused changes with conventional commits (see below).
3. Open a pull request against `main`. CI runs `moon ci` on every PR and must
   pass before merge.
4. Fill in the PR template's summary and acceptance-criteria checklist.

> **Branch-naming note:** this repo uses `feature/...`, a deliberate change from
> the `sven/...` form used in earlier Paigasus repos. Stick to `feature/...`.

## Local development

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
uv) from `.moon/toolchains.yml` on first use — no manual language installs needed.
`proto install` also provides the `buf` CLI, pinned in `.prototools` via a
vendored plugin at `.proto/plugins/buf.toml`.
Per-workspace specifics live in each workspace's `README.md`; the root
[README](./README.md#quickstart) summarizes the overall layout.

### Local development setup (git hooks)

Local git hooks enforce the commit-message and branch-name conventions before
CI sees them. They're managed by [lefthook](https://lefthook.dev) (pinned in
`.prototools`) and run [commitlint](https://commitlint.js.org) from `ts/`.

**Order matters:** run `proto install` *before* installing workspace
dependencies, so the lefthook binary is on `$PATH`:

```bash
proto install                 # installs moon, buf, lefthook
moon run repo:install-hooks   # one-time: writes .git/hooks/{commit-msg,pre-push}
```

`pnpm install` (in `ts/`) also installs the hooks via a `prepare` script, so
contributors who touch `ts/` get them automatically. Pure-Rust / pure-Python
contributors should run the `moon run repo:install-hooks` step above.

- **commit-msg** rejects non-Conventional-Commit messages.
- **pre-push** rejects branches not matching `^feature/[a-z0-9._-]+$`
  (`main` and `dependabot/*` are exempt).
- **Emergency bypass:** `git commit --no-verify` (CI still enforces the rule).
- **GUI git clients** (VS Code, IntelliJ, etc.) often launch with a stripped
  `$PATH`; if commits fail there with "command not found", add your proto shim
  directory (`~/.proto/shims`) to the client's environment.

> Output is buffered for passing tasks (`buffer-only-failure`, set as
> `taskOptions.outputStyle` in `.moon/tasks.yml`). Moon 2.2.5 has no CLI flag
> to override this per invocation; to stream a specific task locally, set
> `options.outputStyle: 'stream'` on the task definition.

## Commit messages

We follow [Conventional Commits](https://www.conventionalcommits.org). Use a
type plus a scope naming the workspace or area:

```
feat(rs): add PRN parser to paigasus-kernel
fix(contracts): correct pagination field number in common/v1
docs(py): document uv workspace setup
```

**Allowed types:** `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `ci`,
`build`, `perf`, `style`, `revert`.

**Allowed scopes:** `rs`, `py`, `ts`, `contracts`, `ci`, `docs`, `deps`,
`release`, `repo`, `claude`, `workspace`. A scope is **required** (use `repo`
or `workspace` for repo-wide changes). Note: a blank line is required before
any commit footer (e.g. `Closes #12`).

> **Maintenance rule (SMA-371):** these type and scope lists are enforced by
> `@paigasus/commitlint-config` (in `ts/packages/commitlint-config/index.cjs`),
> which is the source of truth. When you change either list, update the config
> package **and** this section in the same PR.

## Code conventions

- Every source file starts with an SPDX license header, using the language's
  comment syntax:
  - Rust / TypeScript / Protobuf: `// SPDX-License-Identifier: Apache-2.0`
  - Python: `# SPDX-License-Identifier: Apache-2.0`
- Hand-written config carries no SPDX header. Examples in this repo:
  `moon.yml`, `*.toml`, `*.yaml` / `*.yml`, `*.json`, `*.cjs` / `*.js` config
  (e.g. `commitlint.config.cjs`, ESLint/Prettier config), and dotfiles like
  `.gitignore` / `.editorconfig`. If you're unsure for a new file type, ask in
  the PR — it's almost always config.
- Generated files (lockfiles such as `Cargo.lock` / `uv.lock` /
  `pnpm-lock.yaml`, plus codegen output) carry whatever header the generator
  emits. Don't hand-edit a generated file's header.
- Markdown docs (`README.md`, `CONTRIBUTING.md`, ADRs, design specs) and the
  `LICENSE` file itself carry no SPDX header.
- Per-language formatting and linting are enforced by each workspace's Moon
  tasks; run the workspace's `lint`/`fmt` tasks before pushing once it's set up.

### Task routing by layer (SMA-401)

Per-language task files in `.moon/tasks/` route tasks by **project layer** (`inheritedBy.layers`,
combined with `languages`), so each task attaches only where it belongs:

- **Whole-tree checks** run **once** at the `configuration`-layer workspace roots
  (`py/moon.yml`, `ts/moon.yml`) — their tools read a central config, so one invocation covers
  the whole tree (including root-level files). These are py `lint`/`fmt`/`typecheck`/`test` and
  ts `lint`/`fmt`, defined in `.moon/tasks/python.yml` / `.moon/tasks/typescript.yml`.
- **Per-project tasks** attach to `library`/`application` projects only — they bind to each
  project's own config (`tsconfig.json`, `[project]`) or have no central config. These are py
  `build` and ts `build`/`typecheck`/`test`, defined in `.moon/tasks/python-project.yml` /
  `.moon/tasks/typescript-project.yml`.

The discriminator is *"does a central, cwd-independent config make one whole-tree run correct and
complete?"* — yes for the checks above (py reads `py/pyproject.toml`; ts `lint`/`fmt` read
`ts/eslint.config.js` / `.prettierrc.js`), no for ts `typecheck` (per-`tsconfig.json`) and ts
`test` (no central vitest config; per-package environments). So the workspace roots define no
`build`/`typecheck` (and the ts root no `test`), and those tasks fan out per project — Moon owns
each fan-out graph. A new `library`/`application` project is correct automatically; no per-project
opt-out is needed.

`fileGroups` live in each scoped task file next to the tasks that use them (Moon 2.2.5 does not
propagate global `.moon/tasks.yml` fileGroups to a project that inherits a task from a scoped
file), mirroring `.moon/tasks/rust.yml`.

### Moon project files

Hand-written `moon.yml` files use a fixed top-level field order so diffs
across workspaces stay readable and so generated/scaffolded files line up
with hand-written ones:

1. `$schema`
2. `id` (when present)
3. `layer`
4. `language`
5. `dependsOn`
6. `fileGroups`
7. `tasks`
8. `options`
9. Any remaining fields (alphabetical)

Use `layer:`, not the pre-2.x `type:` — Moon 2.2.5's parser rejects `type:`.
The values in active use are `library` (importable code, e.g. the rust
crates in `rs/crates/libs/` and the py packages in `py/packages/`),
`application` (runnable binary, e.g. `paigasus-gateway-rs`), and
`configuration` (workspace-root project that aggregates child projects,
e.g. `py/moon.yml`), and `tool` (non-language codegen/utility project,
e.g. `contracts`). Moon's full set of seven values is documented in its
[project config docs](https://moonrepo.dev/docs/config/project) — pick
`library` if unsure.

The three scaffold templates under `.moon/templates/{rust,python,typescript}/`
emit this same order, so `moon generate` output is consistent with
hand-written projects (SMA-381).

**App build artifacts (TypeScript):** every `ts/apps/*` that produces a build
artifact MUST define its own Moon `build` task with `outputs:` — as
`paigasus-console` does (`next build` → `outputs: ['.next']`). The `ts` root
excludes the inherited `build`/`typecheck` (SMA-394), so Moon's per-project
tasks own the build graph; a project that only inherits the default `build`
runs `tsc -p tsconfig.json --noEmit`, which type-checks but **emits nothing**.
An app without its own `build` task therefore passes a green build while
producing no artifact. The TypeScript app scaffold
(`.moon/templates/typescript/`, archetype `app`) emits this task for you.

**Config-only TS packages:** a TypeScript _package_ that is not a `tsc`
compilation unit (no `.ts` sources — a CommonJS/JSON config such as a shared
`eslint`/`prettier`/`commitlint` config; `commitlint-config` is the one today)
MUST exclude the inherited per-project `build`/`typecheck`:
`workspace.inheritedTasks.exclude: ['build', 'typecheck']`. Those tasks run
`tsc -p tsconfig.json --noEmit`, which fails `TS5058` with no `tsconfig.json`.
It stays `language: typescript` (so the workspace `lint`/`fmt` at the `ts` root cover its files
and the per-project `test` still attaches) and `layer: library` (importable/published code). The TypeScript scaffold
(`.moon/templates/typescript/`, archetype `config`) emits this block for you,
and the `ts:check-config-only` CI guard fails the build with an actionable
message if a config-only package is added without it.

## Contributor License Agreement

Before your first contribution can be merged you'll be asked to sign a CLA
(automated via a bot — currently being set up). The CLA preserves the project's
ability to relicense and dual-license contributed code; external contributions
can't be merged without it.

## Internal references

For maintainers and contributors with workspace access:

- [Development Guidelines](https://www.notion.so/368830e8fbaa81d297a1f2dacf2f2ff5)
- [Polyglot Monorepo Scoping](https://www.notion.so/368830e8fbaa8101b0ffded7a3de3b53)
- [Architecture Decision Records](https://www.notion.so/368830e8fbaa816cb411c7ee1682c175)
