# Dev setup — verified end-to-end (SMA-363)

What a fresh clone actually required on 2026-06-09, executed for the foundation
acceptance gate (SMA-363) on macOS (Darwin 25.5.0) at commit `b2e5cc1`. Canonical
setup lives in [CONTRIBUTING.md](../CONTRIBUTING.md#local-development); this records
the verified path, observed timings, and gotchas the run surfaced.

## Prerequisites (OS-level)

- `git` with SSH access to GitHub
- [proto](https://moonrepo.dev/proto) (`bash <(curl -fsSL https://moonrepo.dev/install/proto.sh) --yes`),
  with `~/.proto/bin` and `~/.proto/shims` on `PATH`

Everything else (Moon, buf, lefthook, cargo CLIs, and the per-language toolchains —
Rust, Node + pnpm, Python + uv) is installed by the pinned tooling below.

## Verified sequence

Each step ran exactly as written, in this order, with the observed result:

```bash
git clone git@github.com:SMK1085/paigasus-core.git && cd paigasus-core

# Needed for anything affected-graph based (moon ci --base, contracts:breaking):
git fetch --no-tags origin "+refs/heads/main:refs/remotes/origin/main"

proto install                  # moon 2.2.5, buf, lefthook, cargo-deny/machete/nextest — pinned via .prototools
moon run repo:install-hooks    # writes .git/hooks/{commit-msg,pre-push}
moon setup                     # bootstraps Moon-managed toolchains (rust, node/pnpm, python/uv)

moon run :build :test          # whole graph: 26 tasks, all green
moon run :lint :fmt :typecheck repo:deny repo:machete   # warning-free surface: 23 tasks, all green
```

Notes from the run:

- `pnpm --dir ts install --frozen-lockfile` (the step CI runs explicitly) is *not*
  required up front: the ts task graph installs JS deps itself when `node_modules`
  is missing (verified with `MOON_CACHE=off` and `node_modules` removed). Running it
  manually also installs the git hooks via the `prepare` script.
- The lefthook `commit-msg` hook was verified live: a non-Conventional commit
  subject is rejected (commitlint, exit 1) before CI ever sees it.

## Observed timing

Clone → green `moon run :build :test`: **116 seconds**, with a **warm** proto/tool
cache (tools previously downloaded on this machine). This is a recorded observation,
not a guarantee — a cold machine additionally pays one-time toolchain download costs
that CI absorbs via `actions/cache`.

## Gotchas (verified during the gate run)

- `moon ci` requires explicit targets in non-TTY environments (Moon 2.2.5 errors
  with `app::tty::required_id` otherwise).
- `moon query projects --affected` blocks forever reading stdin when stdin is an
  open pipe (scripts, background shells) — redirect with `< /dev/null`.
- Task output is buffered for passing tasks (`buffer-only-failure` in
  `.moon/tasks.yml`); silence while a task runs is normal.
- `cargo nextest` exits non-zero on a workspace with no tests; the task config
  already passes `--no-tests=pass`.
- Affected-graph commands (`moon ci --base origin/main`, `contracts:breaking`'s
  `buf breaking --against …branch=main`) need `origin/main` materialized with full
  history — see the `git fetch` line above; CI does the same in its
  "Materialize main ref" step.
- GUI git clients often strip `PATH`; add `~/.proto/shims` to their environment if
  hooks fail with "command not found".

## NATS (optional — outbox publisher, SMA-471)

The outbox publisher defaults to `backend = "tracing"` and needs nothing. To run the real
JetStream sink locally:

```bash
nats-server -js          # the whole setup; JetStream must be enabled or stream ensure fails
```

then set:

```toml
[outbox.publisher]
backend = "nats"
url     = "nats://127.0.0.1:4222"
```

The service creates and verifies the `IAM_EVENTS` stream at boot, and refuses to start if an
existing stream's `duplicate_window`, storage or subjects are weaker than configured.
Integration tests need no local server — they start their own container.
