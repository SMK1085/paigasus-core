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

Per-workspace setup lives in each workspace's `README.md`; the overall toolchain
and entry points are summarized in the root [README](./README.md#quickstart).
The unified `moon ci` flow becomes available once the workspace-setup issues
land.

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

## Commit messages

We follow [Conventional Commits](https://www.conventionalcommits.org). Use a
type plus a scope naming the workspace or area:

```
feat(rs): add PRN parser to paigasus-kernel
fix(contracts): correct pagination field number in common/v1
docs(py): document uv workspace setup
```

Common types: `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `ci`. Keep
this consistent — changelog automation depends on it.

## Code conventions

- Every source file starts with an SPDX license header, using the language's
  comment syntax:
  - Rust / TypeScript / Protobuf: `// SPDX-License-Identifier: Apache-2.0`
  - Python: `# SPDX-License-Identifier: Apache-2.0`
- Per-language formatting and linting are enforced by each workspace's Moon
  tasks; run the workspace's `lint`/`fmt` tasks before pushing once it's set up.

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
