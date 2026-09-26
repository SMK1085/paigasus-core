# paigasus-core — `rs/`

<!-- Project memory. Claude Code loads this file only when it reads a file in
this directory, so it costs nothing in a session that stays out of rs/.
The root CLAUDE.md holds the repo-wide rules and the two gate-checked blocks. -->

## Cargo, the lockfile and nextest

- `cargo nextest` exits non-zero on a workspace with **no tests** — use `--no-tests=pass`.
- `paigasus-iam`'s Docker-backed suites get their retry budget and container-concurrency cap from
  `rs/.config/nextest.toml` (`profile.default`), so **Moon, `moon run …:test`, and a bare
  `cargo nextest` all pick it up** — but `cargo test` does NOT, since nextest config is
  nextest-only. Don't add `--retries` to a Moon task or a doc: that recreates the
  documented-vs-executed split SMA-521 closed. A test that fails every attempt still reds; one
  that passes on a retry is reported FLAKY. The JUnit report itself is NOT on `profile.default` —
  nextest resolves a profile's report path relative to the shared workspace `target/`, so `moon
  ci`'s 15+ concurrent nextest runs would clobber a report left on `default`. It lives on a
  dedicated `[profile.iam]` instead, selected only by `paigasus-iam-rs:test`'s `args: ['--profile',
  'iam']` — CI uploads it as the `nextest-junit` artifact, but a bare `cargo nextest run -p
  paigasus-iam` writes no report at all.
- `paigasus-iam`'s **Docker-backed** suites (65 of its 69 integration binaries) skip when the
  daemon is unreachable, and that skip is deliberately quiet — nextest discards a passing test's
  stderr and Moon discards a passing task's output, so no message can surface there. What makes
  it visible is `tests/docker_preflight.rs`, a canary that FAILS when Docker is unreachable: a
  Docker-less run yields exactly one red instead of 64 silent passes (SMA-538). The policy itself
  lives once, in `tests/support/docker.rs`, and `repo:iam-docker-policy-single-site` fails if a
  new suite hand-rolls its own copy. Two env vars, both parsing `1`/`true`/`yes` (anything else,
  including `0`, is off — unlike `CI`, which is presence-based):
  `PAIGASUS_REQUIRE_DOCKER=1` turns every suite's skip into a panic, which is what a FILTERED run
  (`--test relay_pg`, `-E 'test(foo)'`) needs, since the canary is not in that filter.
  `PAIGASUS_SKIP_DOCKER=1` restores skipping everywhere including the canary — it is a
  per-invocation escape hatch for a Docker Hub rate limit or a daemon restart, **not** a
  shell-profile setting, and a `moon run` that greened under it leaves a cached PASS that replays
  after Docker returns, so follow it with `moon run … --force`. `CI` outranks both, so no
  workflow-file env var can green a CI run that tested nothing. A container that fails with a
  REACHABLE daemon is a hard failure by default — including `keycloak_e2e`'s 240s startup
  timeout, which used to be a fast local skip — though `PAIGASUS_SKIP_DOCKER=1` still downgrades
  it to a skip, since that hatch is checked before any classification happens. A stray `CI=false` still counts as "CI present" (the
  check is presence-based, not value-based) — clear it with `env -u CI cargo nextest run -p
  paigasus-iam`.
- This repo now has **four** CA-bundle config knobs and they do NOT share semantics. `authn.extra_ca_bundle_path`
  and `upstream.openai.extra_ca_bundle_path` (SMA-558) **ADD** to the trust store — reqwest builds one
  `RootCertStore` by unioning `add_root_certificate` calls with the webpki roots and the platform store, so
  the workspace pins BOTH `rustls-tls` and `rustls-tls-native-roots` (dropping the former is not a
  simplification: reqwest accepts an EMPTY platform store silently, and webpki is the floor that stops a bad
  mount becoming a per-request failure). `outbox.publisher.root_ca_bundle` and the gateway's
  `iam.tls.ca_cert_path` **REPLACE** it. The `extra_` prefix is the marker — a fifth knob must pick a side
  and say which in its doc. Anything in an added bundle becomes an **unconstrained** anchor (no `cA` check),
  so it must contain roots only; a self-signed LEAF works too (put its own cert in the bundle) since rustls
  applies no `cA` check to a trust anchor.
- **Standing rule: A task is in A10's scope when its cargo subcommand COMPILES or LINKS and its
  cwd resolves inside `rs/`. The test is "does this file influence the output", not "is it
  strictly required".**
  `rs/.cargo/config.toml` is now an input of every task that runs cargo from `rs/`: all thirteen
  crates' `build`/`build-release`/`test`/`lint`, the three FFI wrapper tasks, and three `repo:*`
  gates that shell out to cargo (`repo:parity-corpus-drift`, `repo:observability-drift`,
  `repo:nats-permissions`). Editing it selects 61 tasks against 3 before — the 52 crate tasks, the
  3 FFI tasks, and 6 `repo:*` gates (those three, plus `repo:actionlint`, `repo:input-liveness`
  and `repo:publish-metadata`, which select on everything). **59 of those declarations are now
  asserted**: 58 by `repo:affected-smoke`'s A10 (`ci/affected-graph/cargo_moon_parity.py`,
  SMA-599, findings key `a10`), and `paigasus-kernel-py:test` by **A5**, not A10 — it is an FFI
  wrapper task, so the `FFI_TASK_INPUTS` splat already demands the file. A10 DOES derive it (as
  `wrapper`, so the verb test passes); what excludes it is the CWD rule — its blob is
  `uv sync --reinstall-package …` with no cd, and its source dir is `py/packages/paigasus-kernel`. A task is in A10's scope when its cargo subcommand COMPILES or LINKS
  (`CONFIG_SENSITIVE_VERBS`, deliberately NOT A8's `LOCK_RESOLVING_VERBS`) AND its cwd resolves
  inside `rs/` — crate tasks and the `repo:*` gates that reach cargo through their own
  `ci/**/run.sh` alike. `repo:deny` and `repo:machete` are out of scope BY VERB, not on the cwd
  half: `deny` is in `LOCK_RESOLVING_VERBS` and IS derived, but is absent from
  `CONFIG_SENSITIVE_VERBS`, so `_cwd_inside_rs` is never called for it; `machete` is absent from
  `LOCK_RESOLVING_VERBS` entirely and is never derived at all. (`repo:machete` also runs
  `cargo machete rs`, a bare path ARGUMENT, not `--manifest-path`.) The `--manifest-path` fact
  belongs to `repo:deny`, and it is still load-bearing for A10's design decision D2 — MEASURED on
  cargo 1.95.0, a malformed `rs/.cargo/config.toml` fails cwd=rs/ at rc 101 but leaves
  cwd=root+`--manifest-path` at rc 0, so `--manifest-path` does not move cargo's config walk and
  a bare `rs`-containing argument must never confer a cwd. A10 reads moon's RESOLVED inputs, so
  its four inherited lines in `.moon/tasks/rust.yml` (`build`/`build-release`/`test`/`lint`) each
  cover thirteen crates — deleting one reds thirteen tasks. A10 ships with an EMPTY allowlist;
  every exclusion is structural. It is deliberately
  NOT on `fmt`: `cargo fmt --check` neither compiles nor links, so rustflags cannot change its result.
  `repo:wasm-getrandom-free` is excluded for the same kind of reason — it runs `cargo tree`, which
  resolves the dependency graph and never applies rustflags. This REVERSES SMA-546's deliberate
  exclusion, which reasoned that CI is Linux and the darwin flags are inert there. Both are true;
  the criterion changed to "does this file influence the output" rather than "is it strictly
  required", because `rustflags` affect every darwin build from `rs/`. Note maturin injects the
  `-undefined dynamic_lookup` args ITSELF (SMA-578), so the py wheel does not NEED the file — it
  is keyed on it anyway, under the same one rule, which is why `REQUIRED_FFI_TASKS` needs no
  carve-out.
- **A10 enforces the cargo-config rule; nothing enforces the GENERAL one.** A10
  (`ci/affected-graph/cargo_moon_parity.py`, SMA-599) closes the `rs/.cargo/config.toml` case
  specifically, including for a gate that reaches cargo only through its own `ci/**/run.sh` —
  that whole class was outside A8 too until SMA-599. It does not close the general problem: A10
  shares `derive_cargo_tasks`'s VERB LIST, `LOCK_RESOLVING_VERBS`, with A8's derivation, so a
  subcommand outside that list (`cargo llvm-cov`, `insta`, `udeps`, `bloat`) yields an empty
  derivation and stays invisible to A10 too (spec L11). **SMA-605** widened what counts as an
  INVOCATION without touching that verb list: `cargo_matches` merges the literal arm with a
  cargo-NAMED variable in command position (`"$CARGO_BIN" build`) and the `CARGO=<path> <tool>`
  env prefix, the latter carrying wrapper semantics — no flag can ever clear it, since the flag
  reaches the tool and not the cargo behind it. SMA-605 also made script-following TRANSITIVE
  over `source`/`.` statements (cycle-guarded, repo-confined, floored by
  `REQUIRED_SOURCED_SCRIPTS`), which is what finally reaches
  `ci/release-parity/ecosystems/*.sh`; bare `ci/**/*.sh` MENTIONS stay unfollowed, measured at
  six prose edges (comments and pin arrays) and zero true positives. Arm 1 reports ZERO rows on
  the corpus and is labelled forward cover in the code; arm 2 reports one, at
  `ci/release-parity/ecosystems/release-plz.sh:152`, waived because it runs against a
  `mktemp -d` fixture outside the repo. What is still true: A4 covers each
  crate's `lint`/`fmt`, A5 the three derived FFI tasks, and `repo:input-liveness` proves
  DECLARED inputs are live, never that NEEDED ones are declared. A future `repo:*` task can omit
  some OTHER input it reads and nothing reds — check by hand when adding a cargo-invoking gate.
- An **unlocked cargo invocation repairs a truncated `rs/Cargo.lock` in place, mid-run**, and that
  is why five Dependabot PRs (83, 96, 140, 149, 181) merged a truncated lock through a green
  `moon ci`. Measured on PR 181's `72c0ddb52` (176 packages against main's 543, holding 5 of 13
  workspace members): `cargo tree` and `cargo deny` each re-resolved the lock to **548 packages and
  exited 0**, both starting at 06:37:55 — twelve seconds before the first `--locked` task. So
  `paigasus-gateway-rs:lint` and `paigasus-iam-rs:lint` ran `cargo clippy --locked` for real, for
  24s and 72s, against a lock that had already been repaired, and passed. The repaired lock is
  never committed, so `main` keeps the truncated one. Two consequences. A gate that reads the lock
  from the WORKING TREE inside `moon ci` races the repair and is worthless — which is why
  `ci/cargo-lock-integrity/run.sh` is an unconditional **`ci.yml` step placed before the `moon ci`
  step** (pinned by check 8f in `ci/actionlint/run.sh`), not a `repo:*` task. That step runs all
  **three** modes — `--self-test`, `--negative-control`, then the real run, under an explicit
  `set -euo pipefail` — because the bare mode alone is a gate that can lie: with `--locked` deleted
  from `run.sh`'s `cargo metadata` line the command exits **0 and repairs the lock itself**, so the
  gate prints "satisfies every manifest" and becomes the first repairer (MEASURED). Check 8f pins
  both sides — the step's six `ci.yml` lines whole, in order, inside the step's own window, and six
  whole lines inside `run.sh` (`T_CARGO_LOCK_SH_CALL_SITES`), the `cargo metadata --locked` line
  included. It also bans **any `if:` on that step**: a skipped step is a GREEN step, so an `if:`
  switches the guarantee off for every event it excludes, `pull_request` included — which is
  exactly where a Dependabot PR ships a truncated lock. And `cargo deny`
  audits a re-resolved graph whenever the lock does not already satisfy the manifests — not on
  every PR, since cargo rewrites nothing when the lock is consistent, but on exactly the PRs that
  matter. Since SMA-601 every cargo-resolving task passes `--locked`, asserted generically by A8
  (`ci/affected-graph/cargo_moon_parity.py`); the three FFI wrapper tasks cannot, because
  `napi build` exposes no `--locked` and no cargo passthrough, `uv sync` drives maturin with no
  flag path, and `wasm-pack` — which DOES forward `-- --locked` — makes its own unlocked cargo
  call before the forwarded build and repairs the lock there (measured: 176 -> 548 packages,
  exit 0). All three carry `ALLOW_UNLOCKED_CARGO` entries, and A8 demands one for every
  wrapper-matched task even when `--locked` appears elsewhere in its script. A8's
  `LOCK_RESOLVING_VERBS` also lists the verbs that exist to WRITE the lock (`add`, `remove`,
  `generate-lockfile`, `vendor`, `fix`); none is used today, and they are there so a future
  `cargo add` in a Moon task becomes a reviewed `ALLOW_UNLOCKED_CARGO` entry instead of an
  unnoticed repairer. `--locked` proves the lock is
  CONSISTENT with the manifests, not that it is correct: a swapped-but-compatible version or a
  tampered checksum still passes.
- `rs/Cargo.toml`'s `[workspace] members` entries **must be literal crate paths, with no glob**
  (SMA-663). Before SMA-663, the SMA-604 rule allowed at most ONE wildcard level each
  (`crates/libs/*`, not `crates/*/*`). Cargo reads both forms identically — the member set is the
  same 13 crates, measured — but Dependabot's cargo file fetcher cannot expand the two-level form:
  `expand_workspaces` (`cargo/lib/dependabot/cargo/file_fetcher.rb`) lists exactly ONE directory
  level, so for `crates/*/*` it fetches `crates/` and gets `crates/libs`, `crates/services`,
  `crates/bindings`, then drops all three because `File.fnmatch?("crates/*/*", "crates/libs")` is
  false. It finds **zero** members and builds its sandbox from the only in-tree crates still
  reachable — the five declared with `path =` in `[workspace.dependencies]`. Cargo then re-resolves
  that 5-member workspace and rewrites the lock at **176 packages against 543**, which is the
  recurring truncated `rs/Cargo.lock` the entry above describes: SMA-601 gates the SYMPTOM, this is
  the CAUSE. It also reds the job outright — `cargo update -p serde:1.0.228` reports `Locking 0
  packages` there, because serde 1.0.229 needs `serde_core =1.0.229` and `serde_core` is not in the
  `-p` set, so Dependabot raises `Failed to update serde!` and every `cargo in /rs` run from
  2026-08-17 on exited 1 (SMA-604). serde is not special: it is only the first dependency in the
  group that needs a companion package unlocked with it. Nothing else in the repo can see this
  regression — `cargo metadata` is identical either way, and so is every Moon task — so
  `repo:affected-smoke`'s **A9** (`ci/affected-graph/cargo_moon_parity.py`) now asserts it by
  TRANSCRIBING Dependabot's expander rather than restating the rule: it fails if a `members` entry
  resolves to zero members, and separately if any crate directory no entry reaches. Reverting the
  line to `crates/*/*` reds it with 14 rows (MEASURED). Since SMA-663, every entry is a LITERAL
  crate path. So **every new crate** needs its own `members` line. `cargo new` adds it. A9 reds
  if one is missing. A8 and A9 are the two halves of one story: A8 catches a truncated lock once
  it exists, A9 removes the thing that writes one. **No glob at all (SMA-663).** A glob can
  match a dot-directory. `napi build` stages its output in `.<crate>.napi-stage-<random>`
  beside the crate, with no `Cargo.toml`. A concurrent `cargo metadata` then exits 101.
  `repo:affected-smoke`'s **A11** reds on any `*`, `?` or `[` in `members`.
- **The wasm-bindgen family does not move through dependabot (SMA-683).** `js-sys`, `web-sys`
  and `wasm-bindgen-futures` pin `wasm-bindgen` with `=`. Dependabot updates one package at a
  time. `cargo update -p wasm-bindgen` then locks 0 packages (MEASURED, spec M0). Since SMA-680
  the release PR does not move it either. So `wasm-bindgen` stays frozen until a person moves
  the whole family. Do that at a `rust-toolchain.toml` or `wasm-pack` bump, at a `repo:deny`
  advisory for the family, or when a newer `wasm-bindgen` is needed. Use a normal
  `feature/sma-NNN-<slug>` PR.
  Before you start:
  - Put the proto shims on `PATH`.
  - In a fresh worktree, run `proto install` and `pnpm -C ts install`.
  - Check network access (`wasm-pack` downloads `wasm-bindgen-cli`).
  - Unlock 1Password for commit signing.
  ```bash
  ( cd rs && cargo update -p wasm-bindgen -p js-sys -p web-sys -p wasm-bindgen-futures )
  git diff -- rs/Cargo.lock   # the family entries (seven at M0) plus any new dep, crates.io only
  moon run paigasus-kernel-ts:generate-wasm
  moon run paigasus-kernel-ts:test   # the drift gate, before the push
  git add rs/Cargo.lock rs/crates/bindings/paigasus-wasm/paigasus_wasm*
  git commit -m "build(deps): move wasm-bindgen to <version> and regenerate the wasm glue"
  ```
  Push the branch, then open the PR.
  Read the `git diff` BEFORE `generate-wasm`. That task compiles the new proc-macro and build
  scripts on your machine, where your `gh` token and signing agent are available. Run
  `generate-wasm` on ONE host (SMA-634 F12). If the pinned `wasm-pack` does not support the new
  0.2.z, bump it in `.prototools` in the same PR (the invariant above `wasm-bindgen` in
  `rs/Cargo.toml`). Record its error text here when a bump first shows it. It is not measured.
  **The `reqwest` case (INFERRED).** A new `reqwest` can need newer wasm crates. Then a cargo PR
  (usually `cargo-minor-patch`) moves `wasm-bindgen` and fails `committed-wasm.test.ts`. Follow
  these steps in order:
  1. Merge every other open cargo PR.
  2. Let dependabot rebase this PR (its head commit changes), or comment `@dependabot rebase`.
  3. Run `gh pr checkout <N>` (it keeps the `dependabot/*` branch name the pre-push hook needs).
  4. Run `git fetch origin`. Check `git diff origin/main...HEAD -- rs/Cargo.lock`: every changed
     entry must have a crates.io source, and the wasm family entries must be among them. Then
     run `generate-wasm` and the test, commit, and push.
  5. If the branch goes stale again: run the merge-only `update-branch` call, then `git pull`.
     Run `generate-wasm` again only if the merge changed the kernel, the wasm binding, or the
     five artifacts.
  6. For a `Cargo.lock` conflict: merge `origin/main` locally, resolve the lock with
     `( cd rs && cargo update -w )`, then repeat step 4.
  The merge-only `update-branch` call:
  `gh api -X PUT repos/<owner>/<repo>/pulls/<N>/update-branch -f expected_head_sha=<sha>`.
  Never use `--rebase`, `@dependabot recreate` or `[dependabot skip]`. Each one deletes the glue
  commit.
  A conflict or a merge can let git replace the five files. That breaks the pnpm hard link. Run
  `rm -rf ts/node_modules && pnpm -C ts install` after (ts/CLAUDE.md rule).
  Finish before the next Monday 06:00 UTC dependabot run (INFERRED). A new run can supersede a
  grouped PR.

## Container images

- Container images (SMA-500) live behind
  `ci/images/run.sh {build,smoke,all,build-oci,load-oci,rehearse}` and
  `.github/workflows/images.yml`, **not** Moon — a `repo:*` task would have to join `ci.yml`'s
  `T=(…)` array (a `--release` build on every affected PR) or become a `T_EXEMPT` entry.
  The console images (SMA-513) use the same script: `build-console [iam|gateway]` and
  `all-consoles`. The workflow is **not a required check**. So a broken image build makes
  `main` red, not the PR.
- The `pull_request` filter of `images.yml` lists the image build inputs. For `rs/` these are
  `rs/Dockerfile`, `rs/Cargo.{lock,toml}`, `rs/rust-toolchain.toml` and `rs/.dockerignore`. For
  `ts/` these are `ts/Dockerfile`, `ts/.dockerignore`, `ts/pnpm-lock.yaml`,
  `ts/pnpm-workspace.yaml`, `ts/package.json`, `ts/.npmrc`, `ts/apps/*/lib/config.ts`,
  `ts/apps/*/next.config.ts`, `ts/apps/*/package.json`,
  `ts/packages/paigasus-kernel/package.json`,
  `rs/crates/bindings/paigasus-node-bindings/index.js` and
  `rs/crates/bindings/paigasus-node-bindings/index.d.ts`. It also lists `ci/images/**`, the
  workflow, `.prototools` and the two `.proto/plugins/*.toml` files. A PR that changes one of these
  runs the workflow automatically. The rule for a `ts/` entry is in RUNBOOK-containers.md section 1.
  A file that is already a Moon task `input` stays off this filter, even if `ts/Dockerfile` reads
  it too, because a bad edit there already reds the ordinary `moon ci` build — this is why the
  five committed wasm artifacts, `paigasus-wasm/package.json` and
  `paigasus-node-bindings/package.json` are absent, but the napi crate's `index.js`/`index.d.ts`
  are present: those two are Docker-copied by name yet are not Moon `inputs` anywhere, since the
  kernel build task's own `napi build` step regenerates them fresh every run.
- The filter does not list `rs/**` or `ts/**`. A PR that changes `rs/**` or `ts/**` but no
  listed input can still break an image build. Start the workflow manually for such a PR with
  `workflow_dispatch`. (`gh workflow run images.yml --ref <branch>` returns 404 until `images.yml`
  is on `main`.)
- The runtime base is a `chisel cut` of Ubuntu 24.04 into `FROM scratch`. Four traps, all
  measured: `libgcc-s1_libs` is REQUIRED (Rust panic unwinding links `libgcc_s.so.1`) and its
  absence fails at container START, not build; `ca-certificates_data` is the right variant
  (`-with-certs` adds ~120 PEMs nothing reads); there is **no `/etc/passwd`**, so `USER` must be
  numeric; and `chisel cut --root DIR` does not create `DIR`. `/etc/nsswitch.conf` is also absent
  and that is FINE — glibc falls back to a compiled-in `files dns` default and the NSS modules
  ship in `libc6_libs`. The smoke suite pins this by reaching Postgres at a CONTAINER
  HOSTNAME rather than an IP literal; public-name resolution was verified once by hand during
  design and is NOT covered continuously.
- `FROM rust:X.Y.Z` does **not** pin the compiler: `rust-toolchain.toml` is inside the build
  context and rustup honours it over the image, so a channel bump silently changes the compiler
  behind a pinned-looking `FROM`. `rs/Dockerfile` sets `RUSTUP_TOOLCHAIN` and
  `ci/images/run.sh` asserts the two agree. The related invariant — builder glibc ≤ runtime
  glibc (bookworm 2.36 ≤ noble 2.39) — is also asserted there; inverting it fails at container
  start with `GLIBC_2.4x not found`.
- Exec-form `ENTRYPOINT`/`HEALTHCHECK` do **not** expand `ARG`/`ENV`, which is why one
  parameterized `rs/Dockerfile` installs both binaries to the fixed path
  `/usr/local/bin/paigasus-service`. Service identity comes from `paigasus_logging::init`, not
  `argv[0]`.
- `rs/Dockerfile` builds the services with **`cargo auditable`**, which is what makes the image
  SBOM list the Rust crates. MEASURED (SMA-658, 2026-09-20): a plain `cargo build` gives `cargo=0`.
  The OS-package half of the SBOM is still empty and is tracked as SMA-665: syft 1.52.0 reads only
  `/var/lib/dpkg/status` or `.deb` files, and a chisel cut writes neither. The `base-files_chisel`
  slice does NOT help — it writes a chisel-specific manifest that syft cannot read.
