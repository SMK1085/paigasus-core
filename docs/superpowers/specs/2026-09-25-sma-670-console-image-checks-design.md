# SMA-670 — Harden the console image checks in `ci/images/run.sh`

**Linear:** [SMA-670](https://linear.app/smaschek/issue/SMA-670/ops-harden-the-console-image-checks-in-ciimagesrunsh)
**Follows:** SMA-513 PR 1 (PR 279, merged as `8528676e`)
**Date:** 2026-09-25
**Revision:** 3. Revision 2 came after the adversarial challenge (§ 9). Revision 3 corrects S1c,
the place of the `node_modules` filter and the S-INSTALL token strip, from measurements taken for
the plan (`docs/superpowers/plans/2026-09-25-sma-670-console-image-checks.md`, "Deviations").

---

## 1. Problem

`ci/images/run.sh` checks the two console images with `assert_console_pins` (static, over
`ts/Dockerfile`) and `smoke_consoles` (behavioural, against a running container). The PR 279 final
review found four gaps:

1. The runtime image runs a Node version that nothing records. The runtime base can pin only the
   Node major, because distroless publishes no patch-level tags.
2. Three Dockerfile edits pass `assert_console_pins` green:
   - an extra, unpinned `FROM` stage;
   - `RUN pnpm i`, the alias of `pnpm install`, which the `--frozen-lockfile` check does not see;
   - a `PAIGASUS_*` value that a `RUN` or `COPY` step adds. The check reads only `ENV` and `ARG`.
3. The `HEALTHCHECK` smoke row has no timeout. `docker exec … /app/healthcheck.mjs` is unbounded.
   `healthcheck.mjs` calls `fetch` with no timeout of its own, so only undici's default 300 s
   `headersTimeout` bounds it. The goal is a bound below Docker's `--timeout=3s`.
4. The AC 3 text disagrees with itself. The step-4 success line in `smoke_consoles`, RUNBOOK
   section 6 and the PR 279 description say that the AC 3 proof needs the ingress. Spec decision
   D4 of SMA-513 gives AC 3 to the container smoke test.

## 2. Measured facts (2026-09-25)

- M1. `/nodejs/bin/node --version` in the runtime image prints `v24.14.0`. `.prototools` pins
  `node = "24.16.0"`, and the builder `FROM` line is `node:24.16.0-bookworm`. This is a live
  value: a runtime digest refresh changes it.
- M2. `AbortSignal.timeout` is a function in the runtime image's node.
- M3. `Config.Env` of both images holds `PATH`, `SSL_CERT_FILE`, `PORT`, `HOSTNAME` and
  `NODE_ENV`. It holds no `PAIGASUS_*` key.
- M4. A walk of `/app` finds no file whose name starts with `.env` (1367 files in
  `iam-console`, 1324 files in `gateway-console`).
- M5. Kind row R2 (`ts/apps/iam-console/tests/cluster/phase-a/cross-zone.spec.ts`, SMA-513 PR 3,
  PR 296) proves through a real Traefik ingress that both zones hydrate under one origin. Its
  comment cites `docs/ops/RUNBOOK-containers.md:370-374`. Those line numbers are already stale.
  `ts/CLAUDE.md` also cites RUNBOOK line numbers that are stale.
- M6. `.github/workflows/images.yml` runs `ci/images/run.sh all-consoles` on a pull request whose
  paths include `ci/images/**`, `ts/Dockerfile` or `.prototools`. The job installs no pinned
  `node`. The `ubuntu-latest` image has its own `node`, which this design does not use. The job
  prunes every Docker image at its start (`images.yml:101`). `images.yml` is not a required check.
- M7. `x="$(with_deadline 6 true)"` returns after 0 s under `/bin/bash` 3.2.57 and after 6 s
  under Homebrew bash 5.3.15 (both measured). The watchdog's orphan `sleep` keeps the capture
  pipe open under bash 5. So a captured `with_deadline` costs its full deadline on a pass in CI.
  The existing `wait_ready` captures have the same property. This design does not change them.

## 3. Decisions

| # | Topic | Decision |
|---|---|---|
| D1 | Runtime Node version | A new smoke row prints it. A different major, an unparseable version or an unreadable pin is an `::error::`. A different minor or patch is a `::warning::`, and the row stays green. |
| D2 | `HEALTHCHECK` timeout | Both bounds. The smoke row runs `docker exec` under `with_deadline`. `healthcheck.mjs` passes `signal: AbortSignal.timeout(2500)` to `fetch`. A static check holds the signal value below the `HEALTHCHECK --timeout`. |
| D3 | `PAIGASUS_*` outside `ENV`/`ARG` | A static text check and a behavioural image scan. |
| D4 | AC 3 text | Both earlier statements are partly correct. Correct the repo text, and put a comment on PR 279. Do not edit the PR 279 body. |
| D5 | Proof of the mutations | A committed self-test, `ci/images/console-selftest.sh`, runs in `images.yml`. It is advisory in the same way as the rest of `images.yml`, because that workflow is not a required check. |
| D6 | Unpinned images | Every image that the build reads must be one of the two digest-pinned `FROM` images. So: exactly two `FROM` instructions, each one matching a pin regex in the normalised view; every `--from=` value names a stage (`builder`) or the named context (`bindings`); no parser directive. |
| D7 | `PAIGASUS_COMPILED_*` | No exemption in the new text check. `ts/Dockerfile` never writes these values. `createNextConfig` does. |

**Why D1 does not fail on a patch difference.** The repo cannot make the patch equal. The runtime
base tag `nonroot` holds no version, and the digest is the only pin. A red gate that no change in
this repository can make green is noise, not a check. The warning makes the gap visible in every
run log, which is what the issue asks for.

**Why D2 uses 2500 ms.** Docker kills the probe after 3 s. A signal at 2500 ms ends the program
first, with node's own `TimeoutError`, and leaves 500 ms for node's start. Nobody measured the
time of the first `/healthz` answer on a loaded runner. The `--start-period=30s` absorbs slow
answers at start, because Docker does not count failures in that period.

## 4. Design

The change is additive. `crate_for`, `assert_pins`, `build_one`, `smoke` and `assert_base_intact`
do not change. `with_deadline` does not change. `assert_console_pins` and `smoke_consoles` get new
code. The existing checks in them keep their behaviour.

### 4.1 Static checks in `assert_console_pins`

All new checks, except S-DIRECTIVE, read the normalised temp file that the function already writes
(comment lines dropped, `\` continuations joined). They run after the existing `PAIGASUS_`
`ENV`/`ARG` check and before the temp file is removed. Every capture is guarded, and a `grep` rc
greater than 1 gives a separate "could not run" `::error::`, the same shape as the existing
checks. `assert_console_pins` runs under `set -e` in production (`run.sh` dispatch), so an
unguarded capture there stops the script with no named message. The self-test calls it the same
way (§ 5.1).

**S-DIRECTIVE (D6).** On the RAW file: a line before the first non-comment line that matches
`^#[[:space:]]*(syntax|escape|check)[[:space:]]*=` (any letter case) is an error. A `syntax=`
directive makes BuildKit pull an unpinned frontend image, and an `escape=` directive changes how
the normaliser must read the file.

```text
::error::ts/Dockerfile starts with a parser directive (<line>); a '# syntax=' pulls an unpinned BuildKit frontend and '# escape=' changes how this check reads the file — remove it.
```

**S-FROM (gap 2a, D6).** Count the lines of the normalised file that match
`^[[:space:]]*[Ff][Rr][Oo][Mm][[:space:]]`. The count must be 2, and each of those lines must
match one of the two existing pin regexes (the runtime one or the builder one). So the count and
the pins read the same view of the file.

```text
::error::ts/Dockerfile has <N> FROM instruction(s), or a FROM that is not one of the two digest-pinned stages (the node builder and the distroless runtime); an extra or unpinned stage pulls an unpinned image into the build. The FROM lines of the normalised file follow.
```

**S-COPYFROM (D6).** Every `--from=<value>` on the normalised file (a `COPY --from=` or a
`RUN --mount=…,from=`) must have the value `builder` or `bindings`. The values are extracted with
`grep -oE -- '--from=[^[:space:],]+'` and with `from=[^[:space:],]+` inside a `--mount=` argument.

```text
::error::ts/Dockerfile reads from '<value>', which is not the builder stage or the bindings context; a --from=<image> pulls an image that no FROM line pins.
```

**S-INSTALL (gap 2b).** The install extraction changes. The check first extracts every `pnpm`
invocation: from the word `pnpm` to the next `&&`, `;` or `|` (`grep -oE
'(^|[^[:alnum:]_./-])pnpm([[:space:]][^&;|]*)?'`). An `awk` step then splits each invocation into
whitespace-separated tokens, removes `"`, `'`, `` ` ``, `(` and `)` from each token (so
`sh -c "pnpm install"` still counts), and keeps the invocation when a token is exactly `install`, `i`,
`install-test` or `it`. So `pnpm --filter x install`, `pnpm -C ts i` and `pnpm i` at end of line
are installs, and `pnpm info`, `pnpm import`, `pnpm init` and `pnpm exec next build` are not. The
tokenising uses `awk`, not `\b`, `\<` or `\>`, because those are not POSIX ERE and BSD grep and
GNU grep treat them differently. The rest of the check does not change: every install invocation
must hold a bare `--frozen-lockfile` token, and any `--frozen-lockfile=` or
`--no-frozen-lockfile` is rejected. The existing error text changes from `'pnpm install'` to
`'pnpm install'/'pnpm i'`. This is an extension of an existing check, so its mutation reds with
that existing message.

**S-PAIGASUS (gap 2c, text half).** After the existing `ENV`/`ARG` check has passed, no line of
the normalised file may hold `PAIGASUS_` at all. Every `ENV`/`ARG` line with `PAIGASUS_` has
already failed, so this covers `RUN`, `COPY`, `ADD`, `ONBUILD` and a heredoc body line. It also
covers a continuation that the normaliser does not join (a blank line inside an `ENV` value).

```text
::error::ts/Dockerfile names PAIGASUS_ outside an ENV/ARG instruction (a RUN, COPY, ADD or ONBUILD step, or a heredoc body); console config is deployment-varying and must stay runtime-only. The line(s) of the normalised file follow.
```

A comment line cannot trigger it, because normalisation already dropped comment lines. The listed
lines carry no line numbers, because a normalised line number is not a `ts/Dockerfile` line
number.

**S-HCTIMEOUT (D2).** On the raw file: the healthcheck `printf` line must hold exactly one
`AbortSignal.timeout(<digits>)`, and the `HEALTHCHECK` instruction must hold `--timeout=<digits>s`.
The signal value in ms must be less than the Docker timeout in ms. A missing value, or a value
that is not less, is an error.

```text
::error::ts/Dockerfile's healthcheck.mjs fetch has no AbortSignal.timeout(<ms>) below the HEALTHCHECK --timeout (<found>); without it, a server that stops answering hangs the probe until Docker kills it.
```

### 4.2 Smoke rows in `smoke_consoles`

Three new functions. `smoke_consoles` calls each one for each zone, in this exact form:

```bash
console_node_version_row "$app" || ec=1
console_image_config_row "$app" || ec=1
console_healthcheck_row "$name" "$app" "$base_path" "$CONSOLE_HC_DEADLINE" || ec=1
```

`CONSOLE_HC_DEADLINE=20` is a script global beside `CONSOLE_SMOKE_ENV`. The rows do not set
`bad`, so the chunk rows do not depend on them. **Because of the `||`, errexit is off inside
these functions.** Each function must check the rc of every command itself and must not depend on
`set -e`. Each function keeps its JS program in a `local` variable, so it needs no global except
`ROOT`.

`::error::` and `::warning::` lines go to stderr. Green lines go to stdout. This is the existing
convention of the file.

**R-NODE: `console_node_version_row <app>` (gap 1).** It needs only the image, not the running
container, so it runs for each zone whether or not the container started. It captures stdout only
of `docker run --rm --entrypoint /nodejs/bin/node <app>:dev --version` with a guard, keeps the rc,
and parses the first line (`sed -n 1p`). It reads the `.prototools` `node` pin with the same `sed`
as `assert_console_pins`, in a guarded capture.

| Result | Row |
|---|---|
| the pin is empty | `::error::<app>: runtime Node version NOT checked — no node = "X.Y.Z" pin in .prototools.` |
| docker rc not 0 | `::error::<app>: runtime Node version NOT checked — docker exited <rc> …` |
| first line does not match `^v[0-9]+\.[0-9]+\.[0-9]+$` | `::error::<app>: could not parse the runtime Node version from '<line>' …` |
| major is different | `::error::<app>: the runtime image runs Node <v>, but .prototools pins <pin> — a different major …` |
| minor or patch is different | `::warning::<app>: the runtime image runs Node <v>, .prototools pins <pin>. distroless publishes no patch-level tags, so only the major is held.` plus one advice sentence: when the runtime is older, "a later runtime digest refresh closes the gap"; when it is newer, "bump .prototools and the builder FROM line together". A green line follows on stdout. |
| equal | `  <app>: runtime Node <v> matches .prototools` |

**R-CONFIG: `console_image_config_row <app>` (gap 2c, behavioural half).** It needs only the
image, so it also runs whether or not the container started.

1. `docker image inspect --format '{{range .Config.Env}}{{println .}}{{end}}' <app>:dev`, guarded.
   An rc that is not 0 is `::error::<app>: image config NOT checked — docker image inspect exited
   <rc> …`. A line that starts with `PAIGASUS_` is `::error::<app>:dev bakes <KEY> into
   Config.Env …`. The error names the key only, never the value.
2. The image's own node walks `/app` without following symlinks. It prints `walked=<N>` on the
   first line, and then one path for each file whose base name starts with `.env`. The function
   (in bash, not in the walk) ignores a path that holds `/node_modules/`, because Next loads `.env*` only from
   the server's own directory and a dependency can ship an `.env.example`. The walk still counts
   `node_modules` files in `walked`.

| Result | Row |
|---|---|
| docker/node rc not 0 | `::error::<app>: .env scan NOT checked — the walk exited <rc> …` |
| first line is not `walked=<N>`, or N < 100 | `::error::<app>: .env scan walked <N> files under /app — too few to prove anything; the walk read the wrong tree.` |
| one or more paths | `::error::<app>: the image holds .env file(s) under /app outside node_modules; Next loads them at runtime, so a value in them is baked configuration. The paths follow.` |
| no paths | `  <app>: no PAIGASUS_* in Config.Env and no .env file in /app (<N> files walked)` |

M4 measured 1324 and 1367 files, so a floor of 100 is safe.

**R-HEALTH: `console_healthcheck_row <name> <app> <base_path> <deadline>` (gap 3).** The existing
`HEALTHCHECK` row moves into this function. Its pass condition and its existing non-zero message
do not change. It still runs only when the page rendered (`bad` is 0), as today.

- The deadline must be a positive integer. Any other value is an `::error::` and the row fails.
- The call is `with_deadline "$deadline" docker exec "$name" /nodejs/bin/node /app/healthcheck.mjs
  >"$out" 2>&1`, with its rc kept by `|| hc_rc=$?`. The output goes to a `mktemp` file, not to a
  `$( )` capture, because of M7. The file is removed before the function returns. A `mktemp`
  failure is its own `::error::`.
- The function records `$SECONDS` before the call. It reports a timeout only when the rc is 143 or
  137 **and** the elapsed time is at least the deadline. Otherwise a 137 (for example from the OOM
  killer) takes the existing "exited <rc>" message.

```text
::error::<app>: the image's HEALTHCHECK program (/app/healthcheck.mjs) did not finish within <N>s against a server that renders <base_path> — the probe hangs; check the fetch timeout in ts/Dockerfile's healthcheck printf.
```

A killed `docker exec` client can leave the node process in the container. `console_smoke_cleanup`
removes the containers after both zones, and the EXIT trap removes them on any exit. With the
2500 ms signal, the process ends by itself first.

### 4.3 `ts/Dockerfile`

The healthcheck `printf` changes to:

```js
const r = await fetch(`http://127.0.0.1:${process.env.PORT ?? 3000}%s/healthz`, { signal: AbortSignal.timeout(2500) });
process.exit(r.ok ? 0 : 1);
```

When the signal fires, `fetch` rejects with a `TimeoutError`, the top-level await throws, and node
exits 1 and prints the error on stderr. No other instruction changes.

### 4.4 AC 3 text

The correct statement: AC 3 has two halves. The container smoke test (steps 2 to 4) proves the
per-image half (spec D4): each zone emits its asset URLs under its own basePath and serves them
there. Step 4 proves only that a basePath is in effect: the chunk also returns 404 under an unknown
prefix and under no prefix (measured on `iam-console:dev`). So step 4 alone does not prove that the
zones do not collide. Kind row R2 proves the one-origin half: both zones hydrate through one
Traefik ingress.

- The step-4 comment and success line in `smoke_consoles` say this and name R2. The measured
  caveat stays.
- The RUNBOOK section 6 bullet "The zone row of `smoke_consoles` proves only that a basePath is in
  effect" says this and names R2.
- The comment in `cross-zone.spec.ts` and the citations in `ts/CLAUDE.md` cite the RUNBOOK section
  by name, not by line numbers.
- A comment on PR 279 corrects its AC 3 statement and links to R2 and to this PR. The PR 279 body
  does not change.

### 4.5 RUNBOOK section 6

One bullet for each new check: S-DIRECTIVE, S-FROM, S-COPYFROM, the `pnpm i` extension,
S-PAIGASUS, S-HCTIMEOUT, R-NODE (with the current gap and how to close it), R-CONFIG (with its
residual) and R-HEALTH. The existing `PAIGASUS_*` bullet loses the sentence "It does not see a
value that a `RUN` step writes into a file". One sentence records that `ci/kind/run.sh` reports
every `build-console` failure as an infrastructure error (rc 2), so a new static-check failure
shows there as rc 2.

## 5. The self-test: `ci/images/console-selftest.sh`

### 5.1 Shape

- SPDX header, `#!/usr/bin/env bash`, `set -euo pipefail`, `export PROTO_REPORTER=text`. Git mode
  100755.
- Bash 3.2 and bash 5 both run it. So it has no `mapfile`, no `declare -A`, no here-string, no
  pipe into an early-exit reader, no `"${arr[@]}"` on an array that can be empty, no `${v,,}` and
  no `[[ -v ]]`.
- **Loading.** `awk` copies each named function, from `^<name>\(\) \{$` to the next `^\}$`, out of
  `ci/images/run.sh` into a temp file. The functions are `assert_console_pins`, `with_deadline`,
  `console_node_version_row`, `console_image_config_row`, `console_healthcheck_row` and
  `smoke_consoles` (for the call-site rows only; it is never called). The harness runs `bash -n` on
  the temp file, sources it, and checks `declare -F <name>` for each function. Any failure exits
  rc 2 with a message that names the function. The harness never sources `run.sh`, because its
  dispatch code exits before any function runs. `ROOT` is the only global that the harness sets.
- **Call shape.** A row runs its function in a subshell, so an `exit` or a `set -u` failure is a
  FAIL row and does not stop the harness:
  - `assert_console_pins` is called as a plain command under `set -euo pipefail`, which is its
    production shape: `set +e; ( set -euo pipefail; assert_console_pins ) >"$o" 2>"$e"; rc=$?;
    set -e`.
  - A row function is called with `|| rc=$?`, which is its production shape: `rc=0; ( set -uo
    pipefail; fn args ) >"$o" 2>"$e" || rc=$?`.
- **Assertion.** Each row states the rc it expects, a fixed substring that must be in stderr (or
  none), and a fixed substring that must NOT be in stderr (or none). A row passes only when all
  three hold. So a mutation that reds for a different reason is a FAIL.
- **Output.** One `PASS <row>` or `FAIL <row>` line for each row, and exit 1 on any FAIL. On a
  FAIL, the captured output follows with every line prefixed by `    | `. No output line starts
  with `::`, so the Actions runner makes no false annotation.
- Temp files go under one `mktemp -d` directory. An EXIT trap removes it and runs `docker rm -f`
  on the F-row container name.

### 5.2 Static rows

`ROOT` points at a temp tree that holds a copy of the real `ts/Dockerfile` and `.prototools`.
Each mutation is one `sed` or one append on that copy. Each mutation row first checks with `cmp`
that the copy changed, and it is a FAIL named "mutation did not apply" if it did not.

| Row | Mutation | Expect |
|---|---|---|
| S0 | none (the real file) | rc 0 |
| S1 | append `FROM node:latest AS extra` | rc 1, `FROM instruction(s)` |
| S1b | append `from node:latest as extra` | rc 1, `FROM instruction(s)` |
| S1c | a `RUN true \` line is inserted before the builder `FROM` (the digest stays; removing it makes the existing builder-pin check red first) | rc 1, `FROM instruction(s)` |
| S1d | prepend `# syntax=docker/dockerfile:1` | rc 1, `parser directive` |
| S1e | append `COPY --from=alpine:latest /x /y` | rc 1, `which is not the builder stage` |
| S1f | append `RUN --mount=type=bind,from=alpine:latest,target=/x true` | rc 1, `which is not the builder stage` |
| S2 | append `RUN pnpm i --filter x` | rc 1, `carry --frozen-lockfile` |
| S2b | append `RUN pnpm i` | rc 1, `carry --frozen-lockfile` |
| S2c | append `RUN pnpm --filter x install` | rc 1, `carry --frozen-lockfile` |
| S2d | append `RUN pnpm i --frozen-lockfile && pnpm i` | rc 1, `carry --frozen-lockfile` |
| S3 | append `RUN echo PAIGASUS_X=1 > /app/.env` | rc 1, `names PAIGASUS_ outside an ENV/ARG` |
| S4 | append `COPY --from=builder /x /app/PAIGASUS_X` | rc 1, `names PAIGASUS_ outside an ENV/ARG` |
| S5 | append a heredoc: `RUN <<EOF`, `PAIGASUS_X=1`, `EOF` | rc 1, `names PAIGASUS_ outside an ENV/ARG` |
| S6 | remove `, { signal: AbortSignal.timeout(2500) }` | rc 1, `no AbortSignal.timeout` |
| S6b | change `2500` to `3000` | rc 1, `no AbortSignal.timeout` |
| C1 | append `RUN pnpm info x && pnpm import && pnpm init` | rc 0 |
| C2 | append `# PAIGASUS_X in a comment` | rc 0 |

### 5.3 Smoke-row rows

A stub `docker` script is in a harness temp directory. Each row puts that directory first on
`PATH` inside its own subshell only. So the F rows (§ 5.4) use the real `docker`. Environment
variables set the stub's answers: `--version` output and rc, `Config.Env` lines and rc, the walk
output and rc, and a sleep and an rc for `exec`. The stub uses `exec sleep` for the sleep. The
stub writes its argv to a file, and each row compares it with the argv it expects. So an argv
regression fails the self-test, not only the real run.

The N rows use a fixture `.prototools` with a fixed `node = "24.16.0"`, not the real one. So a
Node bump in `.prototools` does not red the self-test.

| Row | Stub state | Expect |
|---|---|---|
| N0 | `v24.16.0` | rc 0, `::warning::` absent |
| N1 | `v24.14.0` | rc 0, `::warning::` and `refresh closes the gap` |
| N1b | `v24.18.0` | rc 0, `::warning::` and `bump .prototools` |
| N2 | `v23.1.0` | rc 1, `a different major` |
| N3 | `garbage` | rc 1, `could not parse` |
| N4 | rc 125 | rc 1, `NOT checked — docker exited 125` |
| N5 | fixture `.prototools` with no `node` line | rc 1, `no node = "X.Y.Z" pin` |
| E0 | clean env, `walked=1300`, no paths | rc 0 |
| E1 | `PAIGASUS_X=1` in env | rc 1, `bakes PAIGASUS_X` |
| E2 | `walked=1300` and `/app/apps/x/.env` | rc 1, `holds .env file` |
| E2b | `walked=1300` and `/app/node_modules/p/.env.example` | rc 0 |
| E3 | inspect rc 1 | rc 1, `image config NOT checked` |
| E4 | walk rc 125 | rc 1, `.env scan NOT checked` |
| E5 | `walked=0` | rc 1, `too few to prove anything` |
| H0 | exec rc 0 | rc 0 |
| H1 | exec sleeps 10, deadline 2 | rc 1, `did not finish within 2s` |
| H2 | exec rc 1 | rc 1, `exited 1` |
| H3 | exec rc 137 at once, deadline 5 | rc 1, `exited 137`, `did not finish` absent |
| H4 | deadline `abc` | rc 1, `positive integer` |

### 5.4 Call-site rows

Row P1 reads the extracted `smoke_consoles` text. It requires each of the three call lines of
§ 4.2 to exist as a whole line (leading space allowed), `|| ec=1` included. Row P2 reads the
extracted `console_healthcheck_row` text and requires the `with_deadline "$deadline" docker exec`
call. Each row has a mutation that deletes the line and must turn the row red.

### 5.5 The real fetch timeout

These rows run in the RUNTIME image that the runtime `FROM` line names (digest included). It is
the node that runs `healthcheck.mjs` in production, and it is smaller than the builder.

1. Extract the `printf` format string of the healthcheck `RUN` line from the real `ts/Dockerfile`.
   If the extraction finds nothing, F0 is a FAIL named "healthcheck printf not found".
2. Render it with the harness bash's `printf` builtin, with the base path `/iam`, into a temp
   file. The format string holds only `\n` and one `%s`, which every `printf` handles the same.
3. `docker pull` the runtime reference, outside any deadline. A pull failure is a FAIL named
   "could not pull the runtime image".
4. Run `docker run --rm --init --network none --name selftest-hc-<pid> -e HC_SRC=<rendered text>
   --entrypoint /nodejs/bin/node <runtime-ref> -e <driver>` under `with_deadline 15`, with its
   output in a temp file. The driver starts a `net` server on `127.0.0.1:3000` that accepts
   connections and never answers. In the `listen` callback it runs
   `import("data:text/javascript," + encodeURIComponent(process.env.HC_SRC))`.
5. **F0** passes only when the rc is exactly 1 **and** the output holds `TimeoutError`. An rc of
   143 or 137 is a FAIL named "the healthcheck hung until the watchdog killed it".
6. **F1** removes `, { signal: AbortSignal.timeout(2500) }` from the rendered text and checks with
   `cmp` that the text changed. It then runs step 4 with `with_deadline 6`, and expects rc 143 or
   137. So F0 and F1 together prove that the signal is what ends the hang.
7. `docker rm -f` the container name after each F row and in the EXIT trap. `docker rmi` the
   runtime reference after the F rows, to give back the disk.

When `docker info` fails, F0 and F1 print `SKIP` with the reason. The harness then exits 1 when
`CI=true`, and 0 otherwise.

### 5.6 CI

A new step, "Console image self-test", runs `ci/images/console-selftest.sh` in `images.yml`
directly before "Build + smoke both consoles". So all rs evidence already exists when it runs,
which is the rule that `images.yml` states for the console step. The path filter already holds
`ci/images/**`, `ts/Dockerfile` and `.prototools`. No Moon task is added, for the reason in
`run.sh`'s header.

## 6. Acceptance

- A1. Each new check has a surgical mutation in the self-test that reds with its own named
  `::error::` substring, and the clean baseline stays green (§ 5.2 to § 5.5).
- A2. `crate_for`, `assert_pins`, `build_one`, `smoke`, `assert_base_intact` and `with_deadline` do
  not change. `git diff` of those six functions is empty.
- A3. `ci/images/run.sh all-consoles` passes on both zones locally, and prints the R-NODE result
  line for each zone.
- A4. `ci/images/console-selftest.sh` passes under `/bin/bash` 3.2 and under Homebrew bash 5, and
  in the `images.yml` step.
- A5. The AC 3 text in `run.sh`, the RUNBOOK, `cross-zone.spec.ts` and `ts/CLAUDE.md` agrees with
  § 4.4, and a comment is on PR 279.

## 7. Out of scope and residuals

- `pnpm add`, `pnpm update` and `npm install` in `ts/Dockerfile` are not checked.
- S-PAIGASUS needs the literal text `PAIGASUS_`. `echo "PAIGASUS""_X=1"`, `printf
  'PAIGASUS\137X'` or a name from `--build-arg` gets past it.
- R-CONFIG does not see a `PAIGASUS_*` value in a file whose name does not start with `.env`, or
  an `.env*` file under `node_modules`.
- S-FROM counts a heredoc body line that starts with `from` (for example a Python
  `from x import y`). The result is a false red, which is visible. The normaliser also drops `#`
  lines and joins `\` lines inside a heredoc body, which Docker does not do.
- R-NODE does not fail on a minor or patch gap (D1).
- The existing `wait_ready` captures of `with_deadline` wait for their full timeout under bash 5
  (M7). This design does not change them.
- The staged-tree parity row still does not gate in CI.
- `images.yml` is not a required check, so a red self-test does not block a merge (D5).

## 8. Files

| File | Change |
|---|---|
| `ci/images/run.sh` | S-DIRECTIVE, S-FROM, S-COPYFROM, S-INSTALL, S-PAIGASUS, S-HCTIMEOUT; three row functions; the `CONSOLE_HC_DEADLINE` global; the three calls and the AC 3 text in `smoke_consoles` |
| `ci/images/console-selftest.sh` | new, mode 100755 |
| `ts/Dockerfile` | the healthcheck `printf` |
| `.github/workflows/images.yml` | the self-test step |
| `docs/ops/RUNBOOK-containers.md` | section 6 |
| `ts/apps/iam-console/tests/cluster/phase-a/cross-zone.spec.ts` | the RUNBOOK citation in a comment |
| `ts/CLAUDE.md` | the RUNBOOK citations |

## 9. Revision 2 changelog (adversarial challenge)

Folded in:

- BLOCKER: F0 passed on a hang. F0 now needs rc 1 and `TimeoutError`, and a watchdog kill is a
  FAIL. F1 checks with `cmp` that its mutation applied. S-HCTIMEOUT adds a static guard.
- The harness now calls each function in its production shape (§ 5.1). § 4.2 states that errexit
  is off inside the row functions.
- Call-site rows P1 and P2 prove that `smoke_consoles` calls the rows (§ 5.4).
- D6 now also covers `--from=<image>`, `RUN --mount=…,from=` and parser directives (S-COPYFROM,
  S-DIRECTIVE). S-FROM checks each `FROM` against the pin regexes in the normalised view.
- S-INSTALL uses `awk` tokens, not word-boundary regex, and covers options before the subcommand
  and `install-test`/`it`. New rows S2b to S2d and control C1.
- The N rows use a fixture `.prototools`.
- R-CONFIG prints `walked=<N>` with a floor of 100, ignores `node_modules`, and has rows E2b, E4
  and E5.
- The F rows use the runtime image, pull it outside the deadline, run with `--init --name`, and
  clean up. They pass the module through `HC_SRC` and a `data:` import.
- The self-test step moves to directly before "Build + smoke both consoles".
- R-HEALTH writes to a temp file, not to a `$( )` capture (M7, measured). It names a timeout only
  when the elapsed time reached the deadline. The deadline is an argument and is validated.
- S-PAIGASUS is now "any `PAIGASUS_` after the ENV/ARG check", with no line numbers.
- R-NODE: guarded pin read, an empty-pin row, stdout-only parse, and advice that depends on which
  side is newer. R-NODE and R-CONFIG no longer need the container to start.
- Harness output is prefixed, so it makes no false annotation. Rows can require that a substring
  is absent. The stub records its argv. The stub's `PATH` is per-row.
- The bash 3.2 trap list, the executable bit, the `ts/CLAUDE.md` citations, the kept AC 3 caveat,
  the kind rc-2 note and corrected statements in § 2 and § 4.2.
- The signal is 2500 ms, not 2000 ms (§ 3, why D2).

Rejected: none. One point is only partly folded in. The challenger asked for S-FROM to skip
heredoc bodies. It is recorded as a visible false-red residual in § 7, because a heredoc-aware
normaliser is a larger change than the risk.
