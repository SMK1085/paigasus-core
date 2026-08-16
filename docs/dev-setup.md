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
- **The `paigasus-iam` integration suites need Docker, and say nothing when they don't have it.**
  Without a daemon each test returns early and reports a pass in under a second. With Docker
  present, small suites legitimately finish in a couple of seconds too, so speed alone isn't the
  tell — a fast *local* run made without `CI=1` is. Run them as `CI=1 cargo nextest run -p
  paigasus-iam` — `CI=1` turns a missing daemon into a hard failure, so you find out immediately
  instead of trusting a green run that executed nothing.
- Retries and the container-concurrency cap for those suites live in `rs/.config/nextest.toml`
  (`profile.default`), so they apply to `moon run`, to `cargo nextest` typed by hand, and to
  anything else that shells out to nextest. `cargo test` bypasses them entirely — prefer `cargo
  nextest` in this repo. The JUnit report nextest writes lives on a separate `profile.iam`, which
  only `paigasus-iam`'s Moon task selects (`--profile iam`); a bare `cargo nextest run -p
  paigasus-iam` still runs under the same retry/concurrency policy but writes no report.

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
# Required for a local broker: the nats backend otherwise demands tls:// AND a credentials_file
# (SMA-493). This one flag waives both — it legalises an unauthenticated broker as well as an
# unencrypted one, which is why it is not called `allow_plaintext`. Never set it in a deployment.
allow_insecure_broker = true
```

For the production shape — a dedicated account, least-privilege subject permissions, TLS and
`.creds` — see [`ops/nats/README.md`](../ops/nats/README.md) and
[`docs/ops/RUNBOOK-nats.md`](./ops/RUNBOOK-nats.md).

The service creates `IAM_EVENTS` if it is absent. If it already exists it is **adopted, never
reshaped** — `get_or_create_stream` does not reconcile, deliberately, so the service can never
silently alter a stream external consumers depend on. Adoption is conditional instead: boot
fails unless the live stream satisfies all five checks, naming the offending field.

| Field | Required |
| -- | -- |
| `retention` | exactly `limits`. `work_queue` deletes a message once one subscriber acks it, and `interest` deletes it once all known observables have — and no consumer ships yet, so on an `interest` stream that condition is vacuous and events are discarded on arrival while every publish still acks. |
| `storage` | exactly `file`. `memory` loses everything on a broker restart. |
| `duplicate_window` | **at least** the configured `duplicate_window_secs` (a wider window is fine). |
| `subjects` | must contain `iam.>`. |
| `max_age` | `0` (unlimited), or greater than `duplicate_window` — JetStream's own constraint. |

`storage` is **never editable in place** on a live JetStream stream, and `retention` cannot be
changed to or from `workqueue` either — both rejected outright by JetStream's Stream Update API.
Since the table above only ever flags a live `retention` of `interest` or `workqueue` as invalid,
a `workqueue` drift needs the same treatment as `storage`: draining or accepting the loss of the
messages the stream holds, deleting it, and letting the service recreate it — plan that as a
maintenance window, not a config tweak. `duplicate_window`, an `interest`-retention drift,
`subjects`, and `max_age` can all be edited in place with `nats stream edit`.

Integration tests need no local server — they start their own container.
