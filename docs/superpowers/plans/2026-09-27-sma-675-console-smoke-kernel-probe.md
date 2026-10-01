# SMA-675 Console Smoke Kernel Probe Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `smoke_consoles` in `ci/images/run.sh` prove that each console image loads the kernel: a seeded session reaches a `(console)` route that answers 200 with a per-run nonce, and a control container with a corrupted wasm chunk proves that the same probe reds when the kernel cannot load.

**Architecture:** Each smoke run starts one Redis sidecar on a per-run Docker network. Each console container gets its `docker create` arguments from ONE function (`console_container_args`). A new route row sends a request with a seeded `__Host-pgs_sid` cookie and asserts 200 plus the nonce. A new control row starts a second container from the same argument list, corrupts every `*paigasus_wasm_bg*.wasm` chunk, and asserts a 5xx plus a kernel line in the log. It removes that container at once. `PAIGASUS_SMOKE_KERNEL_CONTROL=off` (set only in `release.yml`) skips the control. Every new function gets stub rows and call-site pins in `ci/images/console-selftest.sh`.

**Tech Stack:** bash (3.2 and 5), Docker, `redis:7.4-alpine` by digest, curl, GitHub Actions YAML, Markdown.

**Spec:** `docs/superpowers/specs/2026-09-27-sma-675-console-smoke-kernel-probe-design.md` (read it first; this plan cites its sections as D1 to D7, F1 to F18, M1 to M8 and Q1 to Q5).

## Global Constraints

- Worktree: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-675-console-smoke-kernel-probe`, branch `feature/sma-675-console-smoke-kernel-probe`. Start every Bash command with `cd <worktree> &&`. Never commit, check out or reset in `/Users/smaschek/dev/paigasus/paigasus-core` itself.
- Prefix every command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Scratchpad for notes and temporary files: `S=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/34e7e9c8-7c11-4f0c-ae92-59e16bea1f6a/scratchpad`. Do not write measurement notes into the repo.
- Conventional commits, scope `ci` for `ci/` and workflow changes, `docs` for the RUNBOOK, `ts` for `session.ts`. End each message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No `--no-verify`, no `--no-gpg-sign`, no `git commit --amend`, no `git reset`, no force push. If signing fails with "failed to fill whole buffer", stop: 1Password is locked.
- Before each commit, `git -C <worktree> branch --show-current` must print `feature/sma-675-console-smoke-kernel-probe`.
- Do not start background jobs. Do not install host software (no `brew`, no global `npm`).
- `ci/images/run.sh` and `ci/images/console-selftest.sh` must run under `/bin/bash` 3.2 and a bash 5: no `mapfile`, no `declare -A`, no here-string (`<<<`), no heredoc in the self-test, no pipe into an early-exit reader (`grep -q`, `grep -m`, `head`, `awk … exit`), no `${v,,}`, no `[[ -v ]]`, no expansion of an array that can be empty.
- SMA-670 row rule: each new row function is called as `<fn> … || ec=1`, so errexit is off inside it. It checks the rc of every command itself. Every `$( )` capture is guarded (`|| var=""` or `|| rc=$?`) and followed by an explicit check with its own named `::error::`. One zone's failure sets `ec=1` and does not hide the other zone.
- No new row function reads a global. Every run-dependent value is an argument. The ONE exception, decided in this plan: `console_kernel_control_row` appends its own container name to `CONSOLE_SMOKE_NAMES` (with `${CONSOLE_SMOKE_NAMES:-}`), because D4 requires that the control registers its name before `docker create`.
- The self-test loader copies a function from its `<name>() {` line to the next line that is EXACTLY `}`. So no new function may hold a line that is exactly `}` before its own end (for example inside a JS string).
- The Redis image is `redis:7.4-alpine@sha256:858f009f9709ce576febc734aa78b8f6d624b82571f9ddb6bda4377c833b3499` (F9). Two copies, each with a comment that names the other; no check compares them (Q3).
- The seeded record is `SessionRecord` version 2, key `pgs:sess:<sid>`, `PX 600000`, nonce `smoke-<first 12 sid chars>@example.com` in `idTokenClaims.email`, no `refreshToken` field (D2).
- The route row sends ONE `curl -sS -o <body-file> -w '%{http_code} %{redirect_url}' --max-time 30` per try, no `-L`, cookie `__Host-pgs_sid=<sid>`. Only the `requireSession` 3xx is retried: 3 tries, 1 s apart (D3).
- The control row runs only on the `images.yml` paths. `release.yml` sets `PAIGASUS_SMOKE_KERNEL_CONTROL: 'off'` (Q5). Unset or `on` means on; any other value, the empty string included, is a usage error with rc 1.
- The control container is removed with `docker rm -f` as soon as its row finishes, on every exit path (Q2). A failed remove is a `::warning::` only.
- The D4 readiness gate is `<basePath>/healthz`, never `/readyz` (F18).
- New prose (comments, messages, RUNBOOK) is ASD-STE100 Simplified Technical English.
- Commit hook note: `commitlint` runs on commit. `ts/node_modules` exists in this worktree. If the hook reports `commitlint not found`, run `pnpm -C ts install`; do not bypass the hook.

## Review Focus

1. **The two `returnTo` values overlap.** `returnTo=%2Fiam%2F` (the `requireSession` redirect) is a PREFIX of `returnTo=%2Fiam%2Forgs` (the proxy redirect). The row must test the proxy value first and the `requireSession` value only as a SUFFIX of the `Location`. Otherwise a proxy 3xx is retried and reported as a store fault. Task 4 rows K1 (proxy: one request, proxy message) and K2 (requireSession: three requests) pin this.
2. **`redis-cli TIME` microseconds with a leading zero.** bash reads `012345` as an octal number (and `089123` as an error). `console_seed_session` must use `10#`. Task 3 row S3 pins the exact `accessExpiresAt` for `012345`.
3. **An empty `PAIGASUS_SMOKE_KERNEL_CONTROL`.** A step that sets the variable to an empty string must not silently mean "on" or "off". `kernel_control_flag` uses `${VAR-on}`, so only an UNSET variable defaults to on. Task 6 row KC3 pins the usage error.
4. **YAML reads a bare `off` as the boolean false** (YAML 1.1, PyYAML in the repo gates). The `release.yml` value must be the quoted string `'off'`. Task 7 step 2 loads the file with PyYAML and asserts the string.
5. **A control container that leaks after a failure before `docker start`.** A chunk count of 0, a failed `docker cp` or a failed `docker create` must still reach `docker rm -f <control-name>`. Task 5 rows X1, X3 and X9 assert the remove from the stub argv, and row C0 asserts that the EXIT-trap cleanup removes every container before the network.

---

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `ci/images/run.sh` | modify | New constants; new functions `console_new_sid`, `console_container_args`, `console_smoke_redis_start`, `console_seed_session`, `console_kernel_route_row`, `console_kernel_control_row`, `console_kernel_control_probe`, `kernel_control_flag`; `console_smoke_cleanup` removes the network; `smoke_consoles` takes `--kernel-control=on\|off`, uses `docker create` + `docker start` from the shared list, runs the kernel rows; the two console dispatch arms pass the flag; comments per D5. |
| `ci/images/console-selftest.sh` | modify | Extended stub `docker`, new stub `curl`, argv helpers, rows A, SID, R, S, C, K, X, KC, Z, D1, and pins P3 to P16. |
| `ci/kind/manifests/redis.yaml` | modify | One comment line that names the other digest copy (Q3). |
| `.github/workflows/release.yml` | modify | `env:` on "Smoke this service" with `PAIGASUS_SMOKE_KERNEL_CONTROL: 'off'` (Q5). |
| `.github/workflows/images.yml` | modify | Four `pull_request.paths` entries (§6.4). |
| `ts/packages/paigasus-auth/src/core/session.ts` | modify | One comment line on `SessionRecord.version` (§6.5). |
| `docs/ops/RUNBOOK-containers.md` | modify | The kernel-row bullet, the path-filter list and its second rule clause (§6.3). |

---

### Task 1: Take measurements M1 to M8 and fix the row form

This task changes no file in the repo. Its deliverable is `$S/sma-675-measurements.md` (in the scratchpad, NOT in the repo), with one section per measurement. It decides two values that later tasks use: the route row FORM (D3, or D3a if M1 shows no 200) and the literal `CONSOLE_KERNEL_LINE` (M4).

**Files:** none in the repo. Notes: `$S/sma-675-measurements.md`.

**Interfaces:**
- Produces: `ROW_FORM` = `D3` or `D3a`; `KERNEL_LINE` = the exact literal from M4 (expected `CompileError`); `CHUNK_COUNT` per zone (M3); the exact `Location` values of M7 and M8.

- [ ] **Step 1: Build both console images**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-675-console-smoke-kernel-probe && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && ci/images/run.sh build-console
```

Expected: `built iam-console:dev` and `built gateway-console:dev` (the build takes several minutes). If Docker is not running, stop and return a blocker.

- [ ] **Step 2: Start the measurement network, Redis and one console per zone**

Write this script to `$S/m-setup.sh` and run it with `/bin/bash $S/m-setup.sh`. It prints each zone's host port.

```bash
#!/bin/bash
set -euo pipefail
REDIS="redis:7.4-alpine@sha256:858f009f9709ce576febc734aa78b8f6d624b82571f9ddb6bda4377c833b3499"
docker network create --label paigasus.smoke=console m-net
docker run -d --name m-redis --network m-net --label paigasus.smoke=console "$REDIS"
sleep 2
docker exec m-redis redis-cli PING
for z in iam gateway; do
  app="${z}-console"
  docker run -d --name "m-${app}" --network m-net -p 0:3000 \
    --add-host iam:127.0.0.1 --add-host gateway:127.0.0.1 \
    -e PAIGASUS_ZONE="$z" -e 'PAIGASUS_ZONES={"iam":"/iam","gateway":"/gateway"}' \
    -e PAIGASUS_OIDC_ISSUER=https://idp.example.com -e PAIGASUS_OIDC_CLIENT_ID=dummy-client \
    -e PAIGASUS_OIDC_CLIENT_SECRET=dummy-secret -e PAIGASUS_PUBLIC_ORIGIN=https://console.example.com \
    -e 'PAIGASUS_SERVICES={"iam":"http://iam:8080","gateway":"http://gateway:8080"}' \
    -e PAIGASUS_IAM_GRPC_URL=http://iam:9090 \
    -e PAIGASUS_SESSION_STORE=redis -e PAIGASUS_SESSION_REDIS_URL=redis://m-redis:6379 \
    "${app}:dev"
  echo "${app} port: $(docker port "m-${app}" 3000/tcp | sed -n 1p)"
done
```

- [ ] **Step 3: Seed one session per zone (the D2 record)**

Write `$S/m-seed.sh` and run `/bin/bash $S/m-seed.sh <sid>` once per zone with a new sid from `od -An -N32 -tx1 /dev/urandom | tr -d ' \t\n'`. Keep the two sids in the notes.

```bash
#!/bin/bash
set -euo pipefail
sid="$1"
t="$(docker exec m-redis redis-cli TIME)"
secs="$(printf '%s\n' "$t" | sed -n 1p)"; usecs="$(printf '%s\n' "$t" | sed -n 2p)"
now=$((10#$secs * 1000 + 10#$usecs / 1000)); exp=$((now + 600000))
nonce="smoke-${sid:0:12}@example.com"
rec="{\"version\":2,\"rev\":1,\"accessToken\":\"smoke-access-token\",\"accessExpiresAt\":${exp},\"absoluteExpiresAt\":${exp},\"idToken\":\"smoke.id.token\",\"idTokenClaims\":{\"iss\":\"https://idp.example.com\",\"sub\":\"smoke-user\",\"email\":\"${nonce}\"},\"principal\":{\"principalPrn\":null,\"issuer\":\"https://idp.example.com\",\"subject\":\"smoke-user\",\"memberships\":[],\"roleGrants\":[],\"grantsAvailable\":false}}"
docker exec m-redis redis-cli SET "pgs:sess:${sid}" "$rec" PX 600000
echo "nonce=${nonce}"
```

Expected: `OK`, then the nonce.

- [ ] **Step 4: M1 and M2 — the seeded request**

For each zone (`/iam/orgs`, `/gateway/overview`), with its port and sid:

```bash
time curl -sS -o "$S/m1-<zone>.html" -w '%{http_code} %{redirect_url}\n' --max-time 30 \
  -H "Cookie: __Host-pgs_sid=<sid>" "http://127.0.0.1:<port><basePath><probePath>"
grep -c -F -- "smoke-<first 12 sid chars>@example.com" "$S/m1-<zone>.html"
```

Record: the status, the `Location`, the time (M2), and the grep count. Expected: `200`, an empty `Location`, a time under 10 s, a count of 1 or more.

Decision: if BOTH zones give 200 with the nonce, `ROW_FORM=D3`. If either zone does not, `ROW_FORM=D3a` (Q1 option b). Then also record the cause if `docker logs m-<app>` shows it. If the time is above 10 s, raise `--max-time` in Task 4 with a comment; if it is above 30 s, stop and return a blocker (spec §5 M2).

- [ ] **Step 5: M6 — the existing rows with the Redis store**

```bash
curl -s -o /dev/null -w '%{http_code}\n' "http://127.0.0.1:<port><basePath>"
docker exec m-<app> /nodejs/bin/node /app/healthcheck.mjs; echo "rc=$?"
docker logs m-<app> 2>&1 | tail -40
```

Expected: `200`, `rc=0`, and no Redis error in the log (the discovery descriptor cache uses Redis now, F16). If different, investigate before you continue.

- [ ] **Step 6: M7 and M8 — the two 3xx `Location` values**

```bash
# M7: a sid that is not seeded.
curl -sS -o /dev/null -w '%{http_code} %{redirect_url}\n' -H "Cookie: __Host-pgs_sid=$(printf 'f%.0s' $(seq 64))" "http://127.0.0.1:<port>/iam/orgs"
# M8: no cookie.
curl -sS -o /dev/null -w '%{http_code} %{redirect_url}\n' "http://127.0.0.1:<port>/iam/orgs"
```

Expected: M7 is a 3xx whose `Location` ENDS with `returnTo=%2Fiam%2F`. M8 is a 3xx whose `Location` holds `returnTo=%2Fiam%2Forgs`. If the encoding differs (for example `%2f`), use the measured spelling in Task 4 and say so in the notes.

- [ ] **Step 7: M3, M4 and M5 — the broken-chunk container**

```bash
docker run --rm --entrypoint /nodejs/bin/node iam-console:dev -e 'const fs=require("fs");const out=[];const walk=(d)=>{for(const e of fs.readdirSync(d,{withFileTypes:true})){const p=d+"/"+e.name;if(e.isDirectory()){walk(p);}else if(/paigasus_wasm_bg.*\.wasm$/.test(e.name)){out.push(p);}}};walk(process.argv[1]);console.log(out.join("\n"));' /app/apps/iam-console/.next/server/chunks
```

Record the paths and the count per zone (M3; expected at least 1). Then, for iam-console:

```bash
printf '\000asm\377\000\000\000' > "$S/bad.wasm"; chmod 0644 "$S/bad.wasm"
docker create --name m-ctl --network m-net -p 0:3000 --add-host iam:127.0.0.1 --add-host gateway:127.0.0.1 <the same -e flags as step 2 for iam> iam-console:dev
docker cp "$S/bad.wasm" m-ctl:<each path from M3>
docker start m-ctl; sleep 5
docker logs m-ctl > "$S/m4-start.log" 2>&1
curl -s -o /dev/null -w '%{http_code}\n' "http://127.0.0.1:<ctl port>/iam"          # M5
curl -s -o /dev/null -w '%{http_code}\n' "http://127.0.0.1:<ctl port>/iam/healthz"  # M5
/bin/bash "$S/m-seed.sh" <new sid>
curl -sS -o /dev/null -w '%{http_code}\n' -H "Cookie: __Host-pgs_sid=<new sid>" "http://127.0.0.1:<ctl port>/iam/orgs"   # M4
docker logs m-ctl > "$S/m4-full.log" 2>&1
grep -n -F -e CompileError -e WebAssembly "$S/m4-full.log"
```

Record: the M4 status (expected a 5xx), the exact kernel line, and WHEN it appears (in `m4-start.log` = at start by preload, or only in `m4-full.log` = at the request). Set `KERNEL_LINE` to the shortest literal that is specific to the wasm failure and is in the log (expected `CompileError`). Record M5 (expected 200 on `/iam/healthz`; if `/iam` 500s, the note says so, and the row uses `/healthz` only, which is the design already). If M4 gives 200, try a zero-byte file; if it still gives 200, stop and return a blocker (spec §5 M4).

- [ ] **Step 8: Remove the measurement containers**

```bash
docker rm -f m-ctl m-iam-console m-gateway-console m-redis; docker network rm m-net
```

Expected: no container and no network named `m-*` in `docker ps -a` and `docker network ls`.

---

### Task 2: Shared argument list, session id, constants and the self-test stubs

**Files:**
- Modify: `ci/images/run.sh` (constants after `CONSOLE_HC_DEADLINE=20`, new functions after `console_smoke_cleanup`)
- Modify: `ci/kind/manifests/redis.yaml:22` (one comment line above `image:`)
- Test: `ci/images/console-selftest.sh`

**Interfaces:**
- Consumes: `KERNEL_LINE` from Task 1.
- Produces (run.sh):
  - `CONSOLE_SMOKE_REDIS_IMAGE` (string constant), `CONSOLE_KERNEL_LINE` (string constant), `CONSOLE_SMOKE_NETWORK=""` (script-global, read by the cleanup).
  - `console_new_sid` → prints 64 lowercase hex characters and a newline, rc 0; rc 1 and no output on a bad read.
  - `console_container_args <app> <zone> <zones_json> <network> <redis_url> <env_file>` → prints one `docker create` argument per line; rc 1 with a named `::error::` when `<env_file>` is not readable or when `<network>` is set and `<redis_url>` is empty.
- Produces (self-test): the stub `docker` answers `run -d`, the chunk walk, `exec … redis-cli PING|TIME|SET`, `network`, `create`, `start`, `cp`, `port`, `logs`, `rm` from `STUB_*` variables; the stub `curl` answers from `$STUB_CURL_DIR/<n>.{w,body,rc}`; `run_fn` accepts `any` as `<want-argv-file>`; helpers `argv_calls`, `expect_call`, `expect_no_call`, `count_calls`, `curl_reset`, `curl_resp`, `curl_count_is`, `expect_in`, `expect_not_in`.

- [ ] **Step 1: Add the new names to `FUNCS` (the failing test)**

In `ci/images/console-selftest.sh` line 36, replace the `FUNCS=` line with:

```bash
FUNCS="assert_console_pins with_deadline console_node_version_row smoke_consoles console_image_config_row console_healthcheck_row console_smoke_cleanup app_for base_path_for console_probe_path_for console_new_sid console_container_args"
```

- [ ] **Step 2: Run the self-test and see the loader fail**

Run: `cd <worktree> && /bin/bash ci/images/console-selftest.sh; echo rc=$?`
Expected: `console-selftest: infrastructure error (rc=2): no 'console_new_sid() {' line in ci/images/run.sh …` and `rc=2`.

- [ ] **Step 3: Add the constants to `run.sh`**

Directly after the line `CONSOLE_HC_DEADLINE=20` (run.sh:1185), add:

```bash

# SMA-675 D1: the session store of the console smoke. This is the SAME digest as the kind tier's
# Redis. The other copy is ci/kind/manifests/redis.yaml. No check compares the two copies (SMA-675
# Q3). Refresh the two together.
CONSOLE_SMOKE_REDIS_IMAGE="redis:7.4-alpine@sha256:858f009f9709ce576febc734aa78b8f6d624b82571f9ddb6bda4377c833b3499"

# SMA-675 D4 step 5: the line that the kernel control container must log when its wasm chunk is
# corrupt. MEASURED in M4 (the PR description records the full line and when it appears). The row
# searches the FULL log, because Next preloads route modules at start.
CONSOLE_KERNEL_LINE="<KERNEL_LINE from Task 1, expected: CompileError>"
```

Write the measured literal in place of the angle-bracket text (for example `CONSOLE_KERNEL_LINE="CompileError"`).

Directly after the line `CONSOLE_SMOKE_NAMES=""` (run.sh:1147), add:

```bash
# SMA-675 D6: the per-run Docker network of the console smoke. Script-global for the same reason
# as CONSOLE_SMOKE_NAMES: the EXIT trap reads it after smoke_consoles has returned.
CONSOLE_SMOKE_NETWORK=""
```

- [ ] **Step 4: Add `console_new_sid` and `console_container_args`**

Directly after the closing `}` of `console_smoke_cleanup`, add:

```bash

# SMA-675 D2: a session id of 64 lowercase hex characters from /dev/urandom. It prints nothing and
# returns 1 when the read does not give exactly 64 hex characters.
console_new_sid() {
  local sid
  sid="$(od -An -N32 -tx1 /dev/urandom | tr -d ' \t\n')" || sid=""
  case "$sid" in
    ''|*[!0-9a-f]*) return 1 ;;
  esac
  if [ "${#sid}" -ne 64 ]; then return 1; fi
  printf '%s\n' "$sid"
}

# SMA-675 D4 step 2 and spec 6.1: the ONE argument list for `docker create` of a console
# container, the main container and the kernel control container. So the control cannot miss an
# argument that the main container has (for example PAIGASUS_SESSION_REDIS_URL, F17). One
# argument per line on stdout. <env_file> holds the fixed CONSOLE_SMOKE_ENV words, one per line.
# An empty <network> is the D6 fallback: the old memory store and no network flags. The two
# --add-host flags make the IAM and gateway calls fail at once with "connection refused". This
# function reads no global.
console_container_args() {
  local app="$1" zone="$2" zones_json="$3" network="$4" redis_url="$5" env_file="$6" line
  if [ ! -r "$env_file" ]; then
    echo "::error::${app}: the container arguments were NOT built — the fixed env file '${env_file}' is not readable." >&2
    return 1
  fi
  if [ -n "$network" ] && [ -z "$redis_url" ]; then
    echo "::error::${app}: the container arguments were NOT built — the network '${network}' was given with no Redis URL." >&2
    return 1
  fi
  while IFS= read -r line; do
    printf '%s\n' "$line"
  done < "$env_file"
  printf '%s\n' -e "PAIGASUS_ZONE=${zone}" -e "PAIGASUS_ZONES=${zones_json}"
  if [ -n "$network" ]; then
    printf '%s\n' --network "$network" \
      -e "PAIGASUS_SESSION_STORE=redis" -e "PAIGASUS_SESSION_REDIS_URL=${redis_url}" \
      --add-host iam:127.0.0.1 --add-host gateway:127.0.0.1
  else
    printf '%s\n' -e "PAIGASUS_SESSION_STORE=memory"
  fi
  printf '%s\n' -p 0:3000
}
```

- [ ] **Step 5: Add the reverse comment to the kind manifest**

In `ci/kind/manifests/redis.yaml`, directly above the line `          image: redis:7.4-alpine@sha256:858f…` (line 23), add:

```yaml
          # SMA-675 Q3: ci/images/run.sh CONSOLE_SMOKE_REDIS_IMAGE holds a second copy of this digest.
          # No check compares the two copies. Refresh the two together.
```

- [ ] **Step 6: Replace the stub `docker` in the self-test**

In `ci/images/console-selftest.sh`, replace the whole `stub_docker_main() { … }` function (lines 410-440) with:

```bash
stub_docker_main() {
  local a
  for a in "$@"; do
    case "$a" in *$'\n'*) a="<multi-line>" ;; esac
    printf '%s\n' "$a" >> "$STUB_ARGV"
  done
  printf '%s\n' "--" >> "$STUB_ARGV"
  case "${1:-}" in
    run)
      # SMA-675: `docker run -d` starts the Redis sidecar.
      if [ "${2:-}" = "-d" ]; then exit "$STUB_RUND_RC"; fi
      for a in "$@"; do
        if [ "$a" = "--version" ]; then
          if [ -n "$STUB_VERSION_ERR" ]; then printf '%s\n' "$STUB_VERSION_ERR" >&2; fi
          if [ -n "$STUB_VERSION_OUT" ]; then printf '%s\n' "$STUB_VERSION_OUT"; fi
          exit "$STUB_VERSION_RC"
        fi
      done
      # SMA-675: the kernel control row's walk for the wasm chunks.
      for a in "$@"; do
        case "$a" in
          */.next/server/chunks)
            if [ -n "$STUB_CHUNKS_OUT" ]; then printf '%s\n' "$STUB_CHUNKS_OUT"; fi
            exit "$STUB_CHUNKS_RC"
            ;;
        esac
      done
      if [ -n "$STUB_WALK_OUT" ]; then printf '%s\n' "$STUB_WALK_OUT"; fi
      exit "$STUB_WALK_RC"
      ;;
    image)
      if [ -n "$STUB_ENV_OUT" ]; then printf '%s\n' "$STUB_ENV_OUT"; fi
      exit "$STUB_ENV_RC"
      ;;
    exec)
      # SMA-675: `docker exec <redis> redis-cli <command> …`.
      if [ "${3:-}" = "redis-cli" ]; then
        case "${4:-}" in
          PING) printf '%s\n' "$STUB_PING_OUT"; exit 0 ;;
          TIME)
            if [ -n "$STUB_TIME_OUT" ]; then printf '%s\n' "$STUB_TIME_OUT"; fi
            exit "$STUB_TIME_RC"
            ;;
          SET) printf '%s\n' "$STUB_SET_OUT"; exit "$STUB_SET_RC" ;;
        esac
      fi
      if [ "$STUB_EXEC_SLEEP" -gt 0 ]; then exec sleep "$STUB_EXEC_SLEEP"; fi
      exit "$STUB_EXEC_RC"
      ;;
    network) exit "$STUB_NET_RC" ;;
    create)
      if [ "$STUB_CREATE_RC" -eq 0 ]; then echo "stub-container-id"; fi
      exit "$STUB_CREATE_RC"
      ;;
    start) exit "$STUB_START_RC" ;;
    cp) exit "$STUB_CP_RC" ;;
    port) printf '%s\n' "$STUB_PORT_OUT"; exit 0 ;;
    logs)
      if [ -n "$STUB_LOGS_OUT" ]; then printf '%s\n' "$STUB_LOGS_OUT"; fi
      exit 0
      ;;
    rm) exit "$STUB_RM_RC" ;;
  esac
  echo "stub docker: unexpected argv: $*" >&2
  exit 99
}
```

- [ ] **Step 7: Add the stub `curl`**

Directly after the block that writes `$T/stub/docker` and runs `chmod +x "$T/stub/docker"` (line 450), add:

```bash
# SMA-675: the stub curl. Call <n> (counted in $STUB_CURL_DIR/count) answers from
# $STUB_CURL_DIR/<n>.w (printed as the -w output), <n>.body (written to the -o file) and <n>.rc
# (its exit code). A call with no files of its own answers like the last call that has them. It
# records its argv in $STUB_CURL_DIR/argv in the stub docker's format.
stub_curl_main() {
  local n=0 a o="" prev="" k
  if [ -s "$STUB_CURL_DIR/count" ]; then n="$(cat "$STUB_CURL_DIR/count")"; fi
  n=$((n + 1))
  printf '%s\n' "$n" > "$STUB_CURL_DIR/count"
  for a in "$@"; do
    if [ "$prev" = "-o" ]; then o="$a"; fi
    prev="$a"
    printf '%s\n' "$a" >> "$STUB_CURL_DIR/argv"
  done
  printf '%s\n' "--" >> "$STUB_CURL_DIR/argv"
  k="$n"
  while [ "$k" -gt 1 ] && [ ! -e "$STUB_CURL_DIR/$k.w" ]; do k=$((k - 1)); done
  if [ -n "$o" ] && [ "$o" != "/dev/null" ]; then
    : > "$o"
    if [ -e "$STUB_CURL_DIR/$k.body" ]; then cat "$STUB_CURL_DIR/$k.body" > "$o"; fi
  fi
  if [ -e "$STUB_CURL_DIR/$k.w" ]; then printf '%s' "$(cat "$STUB_CURL_DIR/$k.w")"; fi
  if [ -e "$STUB_CURL_DIR/$k.rc" ]; then exit "$(cat "$STUB_CURL_DIR/$k.rc")"; fi
  exit 0
}
{
  printf '%s\n' '#!/usr/bin/env bash'
  declare -f stub_curl_main
  # shellcheck disable=SC2016 # the line is written into the stub file literally
  printf '%s\n' 'stub_curl_main "$@"'
} > "$T/stub/curl"
chmod +x "$T/stub/curl"

# curl_reset <name> — a new, empty answer directory for the stub curl.
curl_reset() {
  STUB_CURL_DIR="$T/curl-$1"
  rm -rf "$STUB_CURL_DIR"
  mkdir -p "$STUB_CURL_DIR"
}

# curl_resp <n> <w-output> <body> <rc> — the answer of stub curl call <n>.
curl_resp() {
  printf '%s' "$2" > "$STUB_CURL_DIR/$1.w"
  printf '%s' "$3" > "$STUB_CURL_DIR/$1.body"
  printf '%s\n' "$4" > "$STUB_CURL_DIR/$1.rc"
}

# curl_count_is <row> <n> — a PASS row <row>-count when the stub curl was called exactly <n> times.
curl_count_is() {
  local got=0
  if [ -s "$STUB_CURL_DIR/count" ]; then got="$(cat "$STUB_CURL_DIR/count")"; fi
  if [ "$got" -eq "$2" ]; then
    say_pass "$1-count"
  else
    say_fail "$1-count" "the stub curl was called ${got} times, expected $2" "$STUB_CURL_DIR/argv"
  fi
}

# argv_calls <argv-file> — one line per stub call, its arguments joined by one space.
argv_calls() {
  awk '$0 == "--" { print line; line = ""; next } { line = (line == "" ? $0 : line " " $0) }' "$1"
}

# expect_call <row> <substring> — a PASS row <row>-argv when one stub docker call of the last
# run_fn holds <substring>. expect_no_call is the reverse.
expect_call() {
  local calls=""
  if [ -e "$T/argv" ]; then calls="$(argv_calls "$T/argv")"; fi
  case "$calls" in
    *"$2"*) say_pass "$1-argv" ;;
    *) say_fail "$1-argv" "no stub docker call holds '$2'" "$T/argv" ;;
  esac
}
expect_no_call() {
  local calls=""
  if [ -e "$T/argv" ]; then calls="$(argv_calls "$T/argv")"; fi
  case "$calls" in
    *"$2"*) say_fail "$1-argv" "a stub docker call holds '$2', and none may" "$T/argv" ;;
    *) say_pass "$1-argv" ;;
  esac
}

# count_calls <prefix> — the number of stub docker calls of the last run_fn that start with
# <prefix>.
count_calls() {
  if [ ! -e "$T/argv" ]; then echo 0; return 0; fi
  argv_calls "$T/argv" | awk -v p="$1" 'index($0, p) == 1 { n++ } END { print n + 0 }'
}

# expect_in <row> <file> <substring> / expect_not_in — a PASS row when <file> holds (or does not
# hold) <substring>. `case`, not grep: no regex and no early-exit reader.
expect_in() {
  local text=""
  if [ -e "$2" ]; then text="$(cat "$2")"; fi
  case "$text" in
    *"$3"*) say_pass "$1" ;;
    *) say_fail "$1" "${2##*/} does not hold '$3'" "$2" ;;
  esac
}
expect_not_in() {
  local text=""
  if [ -e "$2" ]; then text="$(cat "$2")"; fi
  case "$text" in
    *"$3"*) say_fail "$1" "${2##*/} holds '$3', which it must not" "$2" ;;
    *) say_pass "$1" ;;
  esac
}
```

- [ ] **Step 8: Extend `stub_reset` and `run_fn`**

Replace `stub_reset() { … }` with:

```bash
stub_reset() {
  STUB_VERSION_OUT="v24.16.0"; STUB_VERSION_RC=0; STUB_VERSION_ERR=""
  STUB_ENV_OUT="PATH=/usr/bin"; STUB_ENV_RC=0
  STUB_WALK_OUT="walked=1300"; STUB_WALK_RC=0
  STUB_EXEC_SLEEP=0; STUB_EXEC_RC=0
  # SMA-675 defaults: every new docker command succeeds.
  STUB_RUND_RC=0
  STUB_CHUNKS_OUT="/app/apps/iam-console/.next/server/chunks/ssr/x_paigasus_wasm_bg_1.wasm"; STUB_CHUNKS_RC=0
  STUB_PING_OUT="PONG"
  STUB_TIME_OUT="$(printf '%s\n' 1790000000 123456)"; STUB_TIME_RC=0
  STUB_SET_OUT="OK"; STUB_SET_RC=0
  STUB_NET_RC=0; STUB_CREATE_RC=0; STUB_START_RC=0; STUB_CP_RC=0; STUB_RM_RC=0
  STUB_PORT_OUT="0.0.0.0:32768"
  STUB_LOGS_OUT="CompileError: WebAssembly.Module(): expected magic word 00 61 73 6d"
  curl_reset default
  FX_ROOT="$T/fx-node"
  # A row-scoped extra PATH entry, prepended ahead of the docker stub, empty by default. E6 and K7
  # (the grep-rc-2 rows) set it.
  STUB_PATH_EXTRA=""
}
```

Note: `stub_reset` now calls `curl_reset`, so move the whole Step 7 block ABOVE the `stub_reset() {` definition (it is already above if you inserted it after the `chmod +x "$T/stub/docker"` line).

If Task 1 measured a `KERNEL_LINE` other than `CompileError`, write a `STUB_LOGS_OUT` default that holds that literal, and use that literal as `X_LINE` in Task 5.

In `run_fn`, replace the `export` line and the argv check:

```bash
    export PATH STUB_ARGV STUB_VERSION_OUT STUB_VERSION_RC STUB_VERSION_ERR STUB_ENV_OUT STUB_ENV_RC \
      STUB_WALK_OUT STUB_WALK_RC STUB_EXEC_SLEEP STUB_EXEC_RC STUB_RUND_RC STUB_CHUNKS_OUT \
      STUB_CHUNKS_RC STUB_PING_OUT STUB_TIME_OUT STUB_TIME_RC STUB_SET_OUT STUB_SET_RC STUB_NET_RC \
      STUB_CREATE_RC STUB_START_RC STUB_CP_RC STUB_RM_RC STUB_PORT_OUT STUB_LOGS_OUT STUB_CURL_DIR
```

and replace `if [ "$want_argv" = "none" ]; then` with:

```bash
  if [ "$want_argv" = "any" ]; then
    :
  elif [ "$want_argv" = "none" ]; then
```

Also change the header comment of `run_fn` to: `# run_fn <row> <want_rc> <present> <absent> <want-argv-file|none|any> <fn> [<arg>...] — …`.

- [ ] **Step 9: Add the rows for `console_new_sid` and `console_container_args`**

Directly above the line `# --- summary ---…` at the end of the self-test, add:

```bash
# --- SMA-675: console_new_sid and console_container_args (SID, A rows) ------------------------
stub_reset
run_fn SID0 0 "" "::error::" none console_new_sid
SID0_OUT="$(cat "$T/SID0.out")"
case "$SID0_OUT" in
  *[!0-9a-f]*|'') say_fail SID0-shape "the sid is not lowercase hex: '${SID0_OUT}'" ;;
  *) if [ "${#SID0_OUT}" -eq 64 ]; then say_pass SID0-shape; else say_fail SID0-shape "the sid has ${#SID0_OUT} characters, expected 64"; fi ;;
esac

printf '%s\n' -e "PAIGASUS_OIDC_ISSUER=https://idp.example.com" > "$T/a-env"
A_ZONES='{"iam":"/iam","gateway":"/gateway"}'
printf '%s\n' -e "PAIGASUS_OIDC_ISSUER=https://idp.example.com" -e "PAIGASUS_ZONE=iam" \
  -e "PAIGASUS_ZONES=${A_ZONES}" --network smoke-net-1 -e "PAIGASUS_SESSION_STORE=redis" \
  -e "PAIGASUS_SESSION_REDIS_URL=redis://smoke-redis-1:6379" --add-host iam:127.0.0.1 \
  --add-host gateway:127.0.0.1 -p 0:3000 > "$T/a-want-net"
printf '%s\n' -e "PAIGASUS_OIDC_ISSUER=https://idp.example.com" -e "PAIGASUS_ZONE=iam" \
  -e "PAIGASUS_ZONES=${A_ZONES}" -e "PAIGASUS_SESSION_STORE=memory" -p 0:3000 > "$T/a-want-mem"

# a_cmp <row> <want-file> — a PASS row <row>-list when the row's stdout is exactly <want-file>.
a_cmp() {
  if cmp -s "$2" "$T/$1.out"; then
    say_pass "$1-list"
  else
    diff "$2" "$T/$1.out" > "$T/$1.list-diff" 2>&1 || true
    say_fail "$1-list" "a different argument list (< expected, > got)" "$T/$1.list-diff"
  fi
}

stub_reset
run_fn A0 0 "" "::error::" none console_container_args iam-console iam "$A_ZONES" smoke-net-1 "redis://smoke-redis-1:6379" "$T/a-env"
a_cmp A0 "$T/a-want-net"
stub_reset
run_fn A1 0 "" "::error::" none console_container_args iam-console iam "$A_ZONES" "" "" "$T/a-env"
a_cmp A1 "$T/a-want-mem"
stub_reset
run_fn A2 1 "with no Redis URL" "" none console_container_args iam-console iam "$A_ZONES" smoke-net-1 "" "$T/a-env"
stub_reset
run_fn A3 1 "is not readable" "" none console_container_args iam-console iam "$A_ZONES" "" "" "$T/a-env-missing"
```

- [ ] **Step 10: Run the self-test under both bashes**

Run: `cd <worktree> && /bin/bash ci/images/console-selftest.sh; echo rc=$?`, then the same with `/opt/homebrew/bin/bash` (if it exists; on Linux use `bash`).
Expected: every row PASS (F0/F1 may SKIP without a Docker daemon), `rc=0`. The new rows `SID0`, `SID0-shape`, `A0`, `A0-list`, `A1`, `A1-list`, `A2`, `A3` are in the output. If Homebrew bash hangs at about 0% CPU, the host pipe is in the 512-byte state (root CLAUDE.md, SMA-612): record it and rely on `/bin/bash` plus the CI run.

- [ ] **Step 11: Commit**

```bash
cd <worktree> && git branch --show-current && git add ci/images/run.sh ci/images/console-selftest.sh ci/kind/manifests/redis.yaml && git commit -m "feat(ci): add the shared console container argument list for the kernel smoke (SMA-675)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Redis sidecar, seeded session and network cleanup

**Files:**
- Modify: `ci/images/run.sh` (`console_smoke_cleanup`; new functions after `console_container_args`)
- Test: `ci/images/console-selftest.sh`

**Interfaces:**
- Consumes: `CONSOLE_SMOKE_NETWORK` (Task 2).
- Produces:
  - `console_smoke_redis_start <network> <name> <image> <tries>` → rc 0 after `PONG`; rc 1 with a named `::error::` for: a bad `<tries>`, a failed `docker network create`, a failed `docker run -d`, no `PONG` within `<tries>`. The caller registers `<name>` in `CONSOLE_SMOKE_NAMES` and `<network>` in `CONSOLE_SMOKE_NETWORK` BEFORE the call.
  - `console_seed_session <redis-name> <sid>` → rc 0 after the reply `OK`; rc 1 with a named `::error::` when `redis-cli TIME` fails or does not give two integers, or when `SET` does not reply `OK`.
  - `console_smoke_cleanup` removes every registered container, THEN the network.

- [ ] **Step 1: Write the failing rows**

Add `console_smoke_redis_start console_seed_session` to the end of the `FUNCS=` string. Then, directly above `# --- summary ---`, add:

```bash
# --- SMA-675: Redis sidecar, seeded session and cleanup (R, S, C rows) ------------------------
stub_reset
run_fn R0 0 "" "::error::" any console_smoke_redis_start smoke-net-1 smoke-redis-1 redis:stub 2
expect_call R0 "network create --label paigasus.smoke=console smoke-net-1"
expect_call R0 "run -d --name smoke-redis-1 --network smoke-net-1 --label paigasus.smoke=console redis:stub"
stub_reset; STUB_NET_RC=1
run_fn R1 1 "the per-run network smoke-net-1 was not created" "" any console_smoke_redis_start smoke-net-1 smoke-redis-1 redis:stub 2
expect_no_call R1 "run -d"
stub_reset; STUB_RUND_RC=125
run_fn R2 1 "the Redis sidecar smoke-redis-1 did not start" "" any console_smoke_redis_start smoke-net-1 smoke-redis-1 redis:stub 2
stub_reset; STUB_PING_OUT="LOADING"
run_fn R3 1 "did not answer PONG within 2 tries" "" any console_smoke_redis_start smoke-net-1 smoke-redis-1 redis:stub 2
stub_reset
run_fn R4 1 "is not a positive integer" "" none console_smoke_redis_start smoke-net-1 smoke-redis-1 redis:stub x

# The seeded record. TIME 1790000000 s + 123456 us = 1790000000123 ms; + 600000 = 1790000600123.
S_SID="$(printf '0123456789abcdef%.0s' 1 2 3 4)"
stub_reset
run_fn S0 0 "" "::error::" any console_seed_session smoke-redis-1 "$S_SID"
expect_call S0 "exec smoke-redis-1 redis-cli SET pgs:sess:${S_SID} "
expect_call S0 '"email":"smoke-0123456789ab@example.com"'
expect_call S0 '"version":2,"rev":1,'
expect_call S0 '"accessExpiresAt":1790000600123,"absoluteExpiresAt":1790000600123,'
expect_call S0 '"principalPrn":null,'
expect_call S0 " PX 600000"
expect_no_call S0 "refreshToken"
stub_reset; STUB_SET_OUT="ERR wrong number of arguments"
run_fn S1 1 "replied 'ERR wrong number of arguments', not OK" "" any console_seed_session smoke-redis-1 "$S_SID"
stub_reset; STUB_TIME_OUT=""; STUB_TIME_RC=1
run_fn S2 1 "'redis-cli TIME' on smoke-redis-1 exited 1" "" any console_seed_session smoke-redis-1 "$S_SID"
expect_no_call S2 " SET "
# Review Focus 2: a microsecond value with a leading zero is decimal, not octal.
# 012345 us = 12 ms, so 1790000000012 + 600000 = 1790000600012.
stub_reset; STUB_TIME_OUT="$(printf '%s\n' 1790000000 012345)"
run_fn S3 0 "" "::error::" any console_seed_session smoke-redis-1 "$S_SID"
expect_call S3 '"accessExpiresAt":1790000600012,'
stub_reset; STUB_TIME_OUT="$(printf '%s\n' 1790000000 089123)"
run_fn S4 0 "" "::error::" any console_seed_session smoke-redis-1 "$S_SID"
expect_call S4 '"accessExpiresAt":1790000600089,'

# cleanup_case <names> <network> — sets the two cleanup globals, then runs the cleanup.
cleanup_case() {
  CONSOLE_SMOKE_NAMES="$1"
  CONSOLE_SMOKE_NETWORK="$2"
  console_smoke_cleanup
}
printf '%s\n' rm -f smoke-a -- rm -f smoke-b -- network rm smoke-net-1 -- > "$T/argv-cleanup"
stub_reset
run_fn C0 0 "" "" "$T/argv-cleanup" cleanup_case " smoke-a smoke-b" smoke-net-1
printf '%s\n' rm -f smoke-a -- > "$T/argv-cleanup-nonet"
stub_reset
run_fn C1 0 "" "" "$T/argv-cleanup-nonet" cleanup_case " smoke-a" ""
```

C0 compares the full argv file, so it asserts that both `docker rm -f` calls come before `docker network rm`.

- [ ] **Step 2: Run the self-test and see it fail**

Run: `cd <worktree> && /bin/bash ci/images/console-selftest.sh; echo rc=$?`
Expected: the loader reports `no 'console_smoke_redis_start() {' line` and `rc=2`.

- [ ] **Step 3: Make the cleanup remove the network**

Replace `console_smoke_cleanup() { … }` in `run.sh` with:

```bash
console_smoke_cleanup() {
  local n
  for n in $CONSOLE_SMOKE_NAMES; do
    docker rm -f "$n" >/dev/null 2>&1 || true
  done
  CONSOLE_SMOKE_NAMES=""
  # SMA-675 D6: the network goes AFTER the containers. Docker refuses to remove a network that
  # still has endpoints.
  if [ -n "$CONSOLE_SMOKE_NETWORK" ]; then
    docker network rm "$CONSOLE_SMOKE_NETWORK" >/dev/null 2>&1 || true
  fi
  CONSOLE_SMOKE_NETWORK=""
}
```

- [ ] **Step 4: Add `console_smoke_redis_start` and `console_seed_session`**

Directly after the closing `}` of `console_container_args`, add:

```bash

# SMA-675 D6: the per-run network and the Redis sidecar, one for both zones (D7). Both carry the
# label paigasus.smoke=console, so the leftovers of a killed run can be pruned (RUNBOOK). No Redis
# port is published. Readiness is a bounded `redis-cli PING` loop, one second apart; the smoke
# passes 20 tries, the self-test 2. The caller registers <name> in CONSOLE_SMOKE_NAMES and
# <network> in CONSOLE_SMOKE_NETWORK BEFORE this call, so the EXIT trap removes them after an abort.
console_smoke_redis_start() {
  local network="$1" name="$2" image="$3" tries="$4" out rc=0 i=0 pong
  case "$tries" in
    ''|*[!0-9]*|????*) tries="" ;;
  esac
  if [ -z "$tries" ] || [ "$tries" -lt 1 ]; then
    echo "::error::smoke_consoles: the Redis sidecar was NOT started — the try count '$4' is not a positive integer of at most 3 digits." >&2
    return 1
  fi
  out="$(docker network create --label paigasus.smoke=console "$network" 2>&1)" || rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "::error::smoke_consoles: the per-run network ${network} was not created — docker exited ${rc}; its own message follows. The kernel rows are skipped for every zone, and the other rows run on the memory store." >&2
    printf '%s\n' "$out" >&2
    return 1
  fi
  out="$(docker run -d --name "$name" --network "$network" --label paigasus.smoke=console "$image" 2>&1)" || rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "::error::smoke_consoles: the Redis sidecar ${name} did not start from ${image} — docker exited ${rc}; its own message follows. The kernel rows are skipped for every zone, and the other rows run on the memory store." >&2
    printf '%s\n' "$out" >&2
    return 1
  fi
  while [ "$i" -lt "$tries" ]; do
    pong="$(docker exec "$name" redis-cli PING 2>/dev/null)" || pong=""
    if [ "$pong" = "PONG" ]; then
      echo "  Redis sidecar ${name} answers PONG on network ${network}"
      return 0
    fi
    i=$((i + 1))
    if [ "$i" -lt "$tries" ]; then sleep 1; fi
  done
  echo "::error::smoke_consoles: the Redis sidecar ${name} did not answer PONG within ${tries} tries — the kernel rows are skipped for every zone. Its last log lines follow." >&2
  docker logs "$name" 2>&1 | tail -30 >&2 || true
  return 1
}

# SMA-675 D2: one literal SessionRecord (version 2) under pgs:sess:<sid>, valid for 10 minutes,
# with the per-run nonce smoke-<first 12 sid chars>@example.com in the email claim. `now` comes
# from the SIDECAR's clock (redis-cli TIME), not from the host: after a development Mac sleeps,
# the Docker VM clock can differ from the host clock, and the consoles read the VM clock. Ten
# minutes is far above the 30 s refresh skew (F7), and there is no refreshToken field, so no
# refresh and no IdP call can start. `10#` because a microsecond value can have a leading zero,
# and bash reads a leading zero as octal.
# WARNING, INTENTIONAL COUPLING: this literal must pass isSessionRecord in
# ts/packages/paigasus-auth/src/core/session.ts. If the record shape changes, the store deletes
# this record, and the kernel route row reds with its requireSession message. Change the literal
# here too. There is deliberately no second copy of isSessionRecord in bash.
console_seed_session() {
  local redis="$1" sid="$2" t t_rc=0 secs usecs now exp nonce rec reply reply_rc=0
  t="$(docker exec "$redis" redis-cli TIME)" || t_rc=$?
  secs="$(printf '%s\n' "$t" | sed -n 1p)" || secs=""
  usecs="$(printf '%s\n' "$t" | sed -n 2p)" || usecs=""
  case "$secs" in ''|*[!0-9]*) secs="" ;; esac
  case "$usecs" in ''|*[!0-9]*) usecs="" ;; esac
  if [ "$t_rc" -ne 0 ] || [ -z "$secs" ] || [ -z "$usecs" ]; then
    echo "::error::smoke_consoles: the session was NOT seeded — 'redis-cli TIME' on ${redis} exited ${t_rc} and did not give two integers. Its output follows." >&2
    printf '%s\n' "$t" >&2
    return 1
  fi
  now=$((10#$secs * 1000 + 10#$usecs / 1000))
  exp=$((now + 600000))
  nonce="smoke-${sid:0:12}@example.com"
  rec="{\"version\":2,\"rev\":1,\"accessToken\":\"smoke-access-token\",\"accessExpiresAt\":${exp},\"absoluteExpiresAt\":${exp},\"idToken\":\"smoke.id.token\",\"idTokenClaims\":{\"iss\":\"https://idp.example.com\",\"sub\":\"smoke-user\",\"email\":\"${nonce}\"},\"principal\":{\"principalPrn\":null,\"issuer\":\"https://idp.example.com\",\"subject\":\"smoke-user\",\"memberships\":[],\"roleGrants\":[],\"grantsAvailable\":false}}"
  reply="$(docker exec "$redis" redis-cli SET "pgs:sess:${sid}" "$rec" PX 600000)" || reply_rc=$?
  if [ "$reply_rc" -ne 0 ] || [ "$reply" != "OK" ]; then
    echo "::error::smoke_consoles: the session was NOT seeded — 'redis-cli SET' on ${redis} exited ${reply_rc} and replied '${reply}', not OK." >&2
    return 1
  fi
}
```

- [ ] **Step 5: Run the self-test under both bashes**

Run: `cd <worktree> && /bin/bash ci/images/console-selftest.sh; echo rc=$?` (and with a bash 5).
Expected: `rc=0`; the rows R0 to R4, S0 to S4, C0 and C1 and their `-argv` rows PASS. R3 takes about 1 s (one sleep).

- [ ] **Step 6: Commit**

```bash
cd <worktree> && git branch --show-current && git add ci/images/run.sh ci/images/console-selftest.sh && git commit -m "feat(ci): start a Redis sidecar and seed a session for the console smoke (SMA-675)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: The kernel route row (D3, or D3a after M1)

**Files:**
- Modify: `ci/images/run.sh` (new function after `console_seed_session`)
- Test: `ci/images/console-selftest.sh`

**Interfaces:**
- Consumes: `ROW_FORM` and the M7/M8 `Location` spelling from Task 1.
- Produces: `console_kernel_route_row <origin> <base_path> <probe_path> <sid> <name> <tries>` → rc 0 only on 200 with the nonce `smoke-<first 12 sid chars>@example.com` in the body (in the D3a form, also on a `requireSession` 3xx after `<tries>` requests). One named `::error::` per cause otherwise. `<name>` is the main container, for its logs on a 5xx.

- [ ] **Step 1: Write the failing rows**

Add `console_kernel_route_row` to the end of the `FUNCS=` string. Then, directly above `# --- summary ---`, add:

```bash
# --- SMA-675: console_kernel_route_row (K rows) -----------------------------------------------
K_SID="$(printf '0123456789abcdef%.0s' 1 2 3 4)"
K_NONCE="smoke-0123456789ab@example.com"
K_BODY_OK="<html><title>Paigasus IAM</title><script>self.__next_f.push([1,\"{\\\"email\\\":\\\"${K_NONCE}\\\"}\"])</script></html>"
K_BODY_BRAND="<html><title>Paigasus IAM</title></html>"
K_PROXY="302 http://127.0.0.1:32768/iam/auth/login?returnTo=%2Fiam%2Forgs"
K_SESSION="307 http://127.0.0.1:32768/iam/auth/login?returnTo=%2Fiam%2F"
# k_row <row> <want_rc> <present> <absent> [<tries>] — the route row against the stub curl.
k_row() {
  run_fn "$1" "$2" "$3" "$4" any console_kernel_route_row http://127.0.0.1:32768 /iam /orgs "$K_SID" smoke-iam-console-1 "${5:-3}"
}

stub_reset; curl_reset K0; curl_resp 1 "200 " "$K_BODY_OK" 0
k_row K0 0 "" "::error::"
curl_count_is K0 1
expect_in K0-cookie "$STUB_CURL_DIR/argv" "Cookie: __Host-pgs_sid=${K_SID}"
expect_not_in K0-nofollow "$STUB_CURL_DIR/argv" "-L"
# Review Focus 1: the proxy value is tested first and is never retried.
stub_reset; curl_reset K1; curl_resp 1 "$K_PROXY" "" 0
k_row K1 1 "the proxy did not see the session cookie" "requireSession"
curl_count_is K1 1
stub_reset; curl_reset K2; curl_resp 1 "$K_SESSION" "" 0
k_row K2 1 "requireSession found no session after 3 requests" "the proxy did not see"
curl_count_is K2 3
stub_reset; curl_reset K3; curl_resp 1 "302 http://elsewhere.example/x" "" 0
k_row K3 1 "redirected (302) to 'http://elsewhere.example/x'" ""
curl_count_is K3 1
stub_reset; curl_reset K4; curl_resp 1 "404 " "" 0
k_row K4 1 "answered 404" ""
stub_reset; curl_reset K5; curl_resp 1 "500 " "" 0
k_row K5 1 "answered 500" ""
stub_reset; curl_reset K6; curl_resp 1 "000 " "" 7
k_row K6 1 "curl exited 7" ""
stub_reset; curl_reset K7; curl_resp 1 "200 " "$K_BODY_OK" 0; STUB_PATH_EXTRA="$T/stub-grep"
k_row K7 1 "grep exited 2" "holds no"
stub_reset; curl_reset K8; curl_resp 1 "200 " "$K_BODY_BRAND" 0
k_row K8 1 "the body holds no ${K_NONCE}" ""
stub_reset; curl_reset K9; curl_resp 1 "$K_SESSION" "" 0; curl_resp 2 "200 " "$K_BODY_OK" 0
k_row K9 0 "" "::error::"
curl_count_is K9 2
stub_reset; curl_reset K10
k_row K10 1 "is not a positive integer" "" abc
curl_count_is K10 0
```

`$T/stub-grep` is the always-rc-2 grep that row E6 makes earlier in the file, so K7 must stay below E6.

If Task 1 measured a different `returnTo` spelling, change `K_PROXY` and `K_SESSION` to the measured spelling.

- [ ] **Step 2: Run the self-test and see it fail**

Run: `cd <worktree> && /bin/bash ci/images/console-selftest.sh; echo rc=$?`
Expected: `no 'console_kernel_route_row() {' line`, `rc=2`.

- [ ] **Step 3: Write `console_kernel_route_row`**

Directly after the closing `}` of `console_seed_session`, add:

```bash

# SMA-675 D3: the kernel route row. ONE request with the seeded session cookie to the zone's
# (console) route, so the status, the Location and the body come from the same request. It passes
# only on 200 with the per-run nonce in the body. The nonce reaches the HTML only through
# toSessionView(session) in the (console) layout (F13), and that layout imports
# @paigasus/console-core, which evaluates the kernel's wasm at module scope (F1 to F4). The brand
# label is NOT a marker: the root layout puts it in the <title> of every page (F12).
# Deliberately no -L: the redirect is part of the verdict.
# The two 3xx causes have different Location values (F14). proxy.ts writes
# returnTo=<basePath><probePath>; requireSession writes returnTo=<basePath>/. The second one is a
# PREFIX of the first, so the proxy value is tested first, and the requireSession value only as
# the END of the Location. Only the requireSession 3xx is retried (<tries> requests, 1 s apart):
# a slow first Redis connect makes the store read fail (F15), and curl --retry does not retry a
# 3xx.
console_kernel_route_row() {
  local origin="$1" base_path="$2" probe_path="$3" sid="$4" name="$5" tries="$6"
  local url label nonce body w status loc curl_rc=0 g_rc retry rc=1 i=0 p proxy_rt session_rt
  label="${base_path}${probe_path}"
  url="${origin}${label}"
  nonce="smoke-${sid:0:12}@example.com"
  p="${base_path}${probe_path}"
  proxy_rt="returnTo=${p//\//%2F}"
  p="${base_path}/"
  session_rt="returnTo=${p//\//%2F}"
  case "$tries" in
    ''|*[!0-9]*|????*) tries="" ;;
  esac
  if [ -z "$tries" ] || [ "$tries" -lt 1 ]; then
    echo "::error::kernel row ${label}: NOT run — the try count '$6' is not a positive integer of at most 3 digits." >&2
    return 1
  fi
  body="$(mktemp "${TMPDIR:-/tmp}/paigasus-console-kernel.XXXXXX")" || body=""
  if [ -z "$body" ]; then
    echo "::error::kernel row ${label}: NOT run — mktemp failed." >&2
    return 1
  fi
  while :; do
    i=$((i + 1))
    curl_rc=0
    w="$(curl -sS -o "$body" -w '%{http_code} %{redirect_url}' --max-time 30 \
      -H "Cookie: __Host-pgs_sid=${sid}" "$url")" || curl_rc=$?
    status="${w%% *}"
    loc="${w#* }"
    retry=0
    if [ "$curl_rc" -eq 0 ] && [ "$i" -lt "$tries" ]; then
      case "$status" in
        3??)
          case "$loc" in
            *"$proxy_rt"*) ;;
            *"$session_rt") retry=1 ;;
          esac
          ;;
      esac
    fi
    if [ "$retry" -eq 0 ]; then break; fi
    sleep 1
  done
  if [ "$curl_rc" -ne 0 ]; then
    echo "::error::kernel row ${label}: the request failed — curl exited ${curl_rc} (connection failure or timeout), so nothing was proved about the kernel." >&2
  else
    case "$status" in
      200)
        g_rc=0
        grep -F -q -- "$nonce" "$body" || g_rc=$?
        if [ "$g_rc" -eq 0 ]; then
          echo "  kernel row ${label}: 200 with the seeded session's nonce — the (console) layout ran, and it loads the kernel"
          rc=0
        elif [ "$g_rc" -eq 1 ]; then
          echo "::error::kernel row ${label}: answered 200, but the body holds no ${nonce}. So the (console) layout did not render this session: the probe route is not in (console) any more, or a Suspense boundary streams a 200 around a redirect. The brand label is not proof, because every page holds it." >&2
        else
          echo "::error::kernel row ${label}: NOT checked — grep exited ${g_rc} on the response body." >&2
        fi
        ;;
      3??)
        case "$loc" in
          *"$proxy_rt"*)
            echo "::error::kernel row ${label}: redirected (${status}) to '${loc}' — the proxy did not see the session cookie. Check the Cookie header of this request; the session store is not the cause." >&2
            ;;
          *"$session_rt")
            echo "::error::kernel row ${label}: requireSession found no session after ${i} requests (${status} to '${loc}'). Three causes: the seeded record no longer passes isSessionRecord (ts/packages/paigasus-auth/src/core/session.ts; change console_seed_session), the store wiring (PAIGASUS_SESSION_REDIS_URL in console_container_args), or a slow first Redis connect." >&2
            ;;
          *)
            echo "::error::kernel row ${label}: redirected (${status}) to '${loc}', which is neither the proxy login redirect nor the requireSession redirect." >&2
            ;;
        esac
        ;;
      404)
        echo "::error::kernel row ${label}: answered 404 — the route is gone. Next answers from the ROOT not-found, and the (console) layout never runs. Add the zone's route to console_probe_path_for." >&2
        ;;
      5??)
        echo "::error::kernel row ${label}: answered ${status} — the kernel's wasm did not load, or the runtime configuration is wrong. The last log lines of ${name} follow." >&2
        docker logs "$name" 2>&1 | tail -30 >&2 || true
        ;;
      *)
        echo "::error::kernel row ${label}: answered '${status}', not 200." >&2
        ;;
    esac
  fi
  rm -f "$body"
  return "$rc"
}
```

- [ ] **Step 4: Only if `ROW_FORM=D3a` — accept the requireSession 3xx**

Skip this step when Task 1 recorded `ROW_FORM=D3`. Otherwise, in `console_kernel_route_row`, replace the whole `*"$session_rt")` arm of the verdict `case` (the one that prints `requireSession found no session`) with:

```bash
          *"$session_rt")
            # SMA-675 D3a (Q1 option b): M1 showed no 200 for a seeded session with no IAM (see
            # the PR description). This redirect comes from requireSession in the (console)
            # layout, so the layout ran. The kernel control row is what proves that the layout
            # loaded the kernel. The proxy redirect above stays a failure.
            echo "  kernel row ${label}: requireSession redirect (${status}, returnTo=${base_path}/) after ${i} requests — the (console) layout ran (SMA-675 D3a; the kernel control row proves the kernel load)"
            rc=0
            ;;
```

and in the self-test replace the K2 lines with:

```bash
stub_reset; curl_reset K2; curl_resp 1 "$K_SESSION" "" 0
k_row K2 0 "" "::error::"
curl_count_is K2 3
expect_in K2-ok "$T/K2.out" "requireSession redirect (307, returnTo=/iam/) after 3 requests"
```

- [ ] **Step 5: Run the self-test under both bashes**

Run: `cd <worktree> && /bin/bash ci/images/console-selftest.sh; echo rc=$?` (and with a bash 5).
Expected: `rc=0`; K0 to K10 and their `-count` rows PASS; K2 takes about 2 s.

- [ ] **Step 6: Commit**

```bash
cd <worktree> && git branch --show-current && git add ci/images/run.sh ci/images/console-selftest.sh && git commit -m "feat(ci): add the kernel route row to the console smoke (SMA-675)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: The kernel control row (D4)

**Files:**
- Modify: `ci/images/run.sh` (two new functions after `console_kernel_route_row`)
- Test: `ci/images/console-selftest.sh`

**Interfaces:**
- Consumes: `console_new_sid`, `console_seed_session` (Tasks 2, 3); `CONSOLE_SMOKE_NAMES`.
- Produces:
  - `console_kernel_control_row <app> <image> <base_path> <probe_path> <redis_name> <args_file> <kernel_line>` → rc 0 only on a 5xx AND `<kernel_line>` in the FULL control log. It registers `smoke-<app>-nokernel-$$` in `CONSOLE_SMOKE_NAMES`, and it calls `docker rm -f` on that name before it returns, on every path.
  - `console_kernel_control_probe <ctl-name> <tmp-dir> <the seven arguments above>` (the inner work; called only by the row).

- [ ] **Step 1: Write the failing rows**

Add `console_kernel_control_row console_kernel_control_probe` to the end of the `FUNCS=` string. Then, directly above `# --- summary ---`, add:

```bash
# --- SMA-675: console_kernel_control_row (X rows) ---------------------------------------------
# Every X row asserts that the row removes its control container before it returns (SMA-675 Q2).
X_CTL="smoke-iam-console-nokernel-$$"
X_LINE="CompileError"
printf '%s\n' -e "PAIGASUS_ZONE=iam" --network smoke-net-1 -p 0:3000 > "$T/x-args"
X_CHUNK_A="/app/apps/iam-console/.next/server/chunks/ssr/a_paigasus_wasm_bg_1.wasm"
X_CHUNK_B="/app/apps/iam-console/.next/server/chunks/ssr/b_paigasus_wasm_bg_2.wasm"
# x_row <row> <want_rc> <present> <absent> [<kernel-line>] — the control row against the stubs.
x_row() {
  run_fn "$1" "$2" "$3" "$4" any console_kernel_control_row iam-console paigasus-iam-console:dev /iam /orgs smoke-redis-1 "$T/x-args" "${5-$X_LINE}"
  expect_call "$1-rm" "rm -f ${X_CTL}"
}

stub_reset; curl_reset X0; curl_resp 1 "200" "" 0; curl_resp 2 "500" "" 0
x_row X0 0 "" "::error::"
expect_call X0-create "create --name ${X_CTL} -e PAIGASUS_ZONE=iam --network smoke-net-1 -p 0:3000 paigasus-iam-console:dev"
expect_call X0-cp "cp "
expect_call X0-cp-target "${X_CTL}:${STUB_CHUNKS_OUT}"
expect_call X0-seed "exec smoke-redis-1 redis-cli SET pgs:sess:"
expect_in X0-healthz "$STUB_CURL_DIR/argv" "http://127.0.0.1:32768/iam/healthz"
expect_in X0-probe "$STUB_CURL_DIR/argv" "http://127.0.0.1:32768/iam/orgs"
stub_reset; STUB_CHUNKS_OUT=""; curl_reset X1
x_row X1 1 "found 0 *paigasus_wasm_bg*.wasm files" ""
expect_no_call X1 "create "
stub_reset; STUB_CHUNKS_OUT="$(printf '%s\n' "$X_CHUNK_A" "$X_CHUNK_B")"; curl_reset X2; curl_resp 1 "200" "" 0; curl_resp 2 "500" "" 0
x_row X2 0 "" "::error::"
X2_CP="$(count_calls "cp ")"
if [ "$X2_CP" -eq 2 ]; then say_pass X2-cp-count; else say_fail X2-cp-count "${X2_CP} docker cp calls, expected 2" "$T/argv"; fi
stub_reset; STUB_CP_RC=1; curl_reset X3
x_row X3 1 "docker cp exited 1" ""
expect_no_call X3 "start "
stub_reset; curl_reset X4; curl_resp 1 "200" "" 0; curl_resp 2 "200" "" 0
x_row X4 1 "answered 200 with every wasm chunk corrupted" ""
stub_reset; curl_reset X5; curl_resp 1 "200" "" 0; curl_resp 2 "302" "" 0
x_row X5 1 "answered '302'" ""
stub_reset; STUB_LOGS_OUT="AuthConfigError: PAIGASUS_SESSION_REDIS_URL is required"; curl_reset X6; curl_resp 1 "200" "" 0; curl_resp 2 "500" "" 0
x_row X6 1 "for a reason that is not the kernel" ""
stub_reset; curl_reset X7
x_row X7 1 "the kernel line is empty" "" ""
stub_reset; curl_reset X8; curl_resp 1 "503" "" 0
x_row X8 1 "never answered 200 on /iam/healthz" ""
stub_reset; STUB_CREATE_RC=125; curl_reset X9
x_row X9 1 "was not created from paigasus-iam-console:dev" ""
stub_reset; STUB_RM_RC=1; curl_reset X10; curl_resp 1 "200" "" 0; curl_resp 2 "500" "" 0
x_row X10 0 "::warning::iam-console: the kernel control container ${X_CTL} was not removed" "::error::"
```

X10 asserts that a failed remove is only a warning: the verdict stays rc 0.

- [ ] **Step 2: Run the self-test and see it fail**

Run: `cd <worktree> && /bin/bash ci/images/console-selftest.sh; echo rc=$?`
Expected: `no 'console_kernel_control_row() {' line`, `rc=2`.

- [ ] **Step 3: Write the two functions**

Directly after the closing `}` of `console_kernel_route_row`, add:

```bash

# SMA-675 D4: the kernel control row. It proves that the kernel route row can see a kernel
# failure. It runs on every images.yml smoke run, so the control cannot go stale. The work is in
# console_kernel_control_probe; this function owns the control container's life. It registers the
# name BEFORE docker create, so the EXIT trap removes the container after an abort, and it keeps
# the name registered (a `docker rm -f` of a removed container is harmless). SMA-675 Q2: it
# removes the container as soon as the row finishes, on every path, to limit the disk use of the
# images.yml job. A failed remove is a warning only; the EXIT-trap cleanup removes it again.
# This is the one row function that writes a global (the cleanup registry), because D4 requires
# the registration before docker create.
console_kernel_control_row() {
  local app="$1" ctl rc=0 rm_out rm_rc=0 tmp
  ctl="smoke-${app}-nokernel-$$"
  CONSOLE_SMOKE_NAMES="${CONSOLE_SMOKE_NAMES:-} ${ctl}"
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/paigasus-console-control.XXXXXX")" || tmp=""
  if [ -z "$tmp" ]; then
    echo "::error::${app}: kernel control row NOT run — mktemp failed." >&2
    return 1
  fi
  console_kernel_control_probe "$ctl" "$tmp" "$@" || rc=$?
  rm_out="$(docker rm -f "$ctl" 2>&1)" || rm_rc=$?
  if [ "$rm_rc" -ne 0 ]; then
    echo "::warning::${app}: the kernel control container ${ctl} was not removed (docker exited ${rm_rc}); the EXIT-trap cleanup removes it again." >&2
    printf '%s\n' "$rm_out" >&2
  fi
  rm -rf "$tmp"
  return "$rc"
}

# SMA-675 D4 steps 1 to 5, called only by console_kernel_control_row.
# 1. The image's OWN node (the runtime base has no shell) walks the server chunks and prints every
#    file whose name matches *paigasus_wasm_bg*.wasm (the glob of ts/Dockerfile and moon.yml). At
#    least one is required, and EVERY match is corrupted, so a build that writes one file per
#    layer cannot give a false control.
# 2. docker create from the SAME argument list as the main container (console_container_args).
# 3. docker cp of an 8-byte file (the wasm magic and a wrong version) over each chunk. The file is
#    0644: docker cp makes it root-owned, and a 0600 file then fails with EACCES, not with a
#    compile error.
# 4. docker start, wait for <basePath>/healthz (the public route loads no kernel; /readyz answers
#    503 here, F18), seed a new sid, and send the kernel row's request.
# 5. Pass only on a 5xx AND the kernel line in the FULL log: Next preloads route modules at start,
#    so the compile error can come before the request.
console_kernel_control_probe() {
  local ctl="$1" tmp="$2" app="$3" image="$4" base_path="$5" probe_path="$6" redis="$7" args_file="$8" kernel_line="$9"
  local out rc=0 n path line port origin status sid code logs_rc=0 g_rc=0 cargs
  cargs=()
  local chunk_js='
const fs = require("fs");
const out = [];
const walk = (d) => {
  for (const e of fs.readdirSync(d, { withFileTypes: true })) {
    const p = d + "/" + e.name;
    if (e.isDirectory()) { walk(p); } else if (/paigasus_wasm_bg.*\.wasm$/.test(e.name)) { out.push(p); }
  }
};
walk(process.argv[1]);
console.log(out.join("\n"));
'
  if [ -z "$kernel_line" ]; then
    echo "::error::${app}: kernel control row NOT run — the kernel line is empty; set CONSOLE_KERNEL_LINE to the M4 literal." >&2
    return 1
  fi
  out="$(docker run --rm --entrypoint /nodejs/bin/node "$image" -e "$chunk_js" "/app/apps/${app}/.next/server/chunks")" || rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "::error::${app}: kernel control row NOT run — the chunk walk exited ${rc} on ${image}." >&2
    return 1
  fi
  printf '%s\n' "$out" | sed '/^$/d' > "$tmp/chunks"
  n="$(wc -l < "$tmp/chunks" | tr -d ' ')" || n=""
  case "$n" in ''|*[!0-9]*) n=0 ;; esac
  if [ "$n" -lt 1 ]; then
    echo "::error::${app}: kernel control row NOT run — found 0 *paigasus_wasm_bg*.wasm files under /app/apps/${app}/.next/server/chunks in ${image}; at least 1 is required. A bundler that inlines the wasm into a JS chunk also gives 0." >&2
    return 1
  fi
  printf '\000asm\377\000\000\000' > "$tmp/bad.wasm"
  chmod 0644 "$tmp/bad.wasm"
  while IFS= read -r line; do
    cargs[${#cargs[@]}]="$line"
  done < "$args_file"
  out="$(docker create --name "$ctl" "${cargs[@]}" "$image" 2>&1)" || rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "::error::${app}: the kernel control container ${ctl} was not created from ${image} — docker exited ${rc}; its own message follows." >&2
    printf '%s\n' "$out" >&2
    return 1
  fi
  while IFS= read -r path; do
    out="$(docker cp "$tmp/bad.wasm" "${ctl}:${path}" 2>&1)" || rc=$?
    if [ "$rc" -ne 0 ]; then
      echo "::error::${app}: kernel control row NOT run — docker cp exited ${rc} on ${path}; its own message follows." >&2
      printf '%s\n' "$out" >&2
      return 1
    fi
  done < "$tmp/chunks"
  out="$(docker start "$ctl" 2>&1)" || rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "::error::${app}: the kernel control container ${ctl} did not start — docker exited ${rc}; its own message follows." >&2
    printf '%s\n' "$out" >&2
    return 1
  fi
  port="$(docker port "$ctl" 3000/tcp | sed -n 1p)" || port=""
  port="${port##*:}"
  case "$port" in
    ''|*[!0-9]*)
      echo "::error::${app}: kernel control row NOT run — no host port for ${ctl} ('${port}'). Its last log lines follow." >&2
      docker logs "$ctl" 2>&1 | tail -30 >&2 || true
      return 1
      ;;
  esac
  origin="http://127.0.0.1:${port}"
  status="$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 --retry 20 --retry-delay 1 \
    --retry-all-errors "${origin}${base_path}/healthz")" || status=""
  if [ "$status" != "200" ]; then
    echo "::error::${app}: the kernel control container never answered 200 on ${base_path}/healthz (HTTP ${status:-no response}), so it cannot tell 'started' from 'broken'. Its last log lines follow." >&2
    docker logs "$ctl" 2>&1 | tail -30 >&2 || true
    return 1
  fi
  sid="$(console_new_sid)" || sid=""
  if [ -z "$sid" ]; then
    echo "::error::${app}: kernel control row NOT run — no session id could be read from /dev/urandom." >&2
    return 1
  fi
  if ! console_seed_session "$redis" "$sid"; then
    return 1
  fi
  code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 \
    -H "Cookie: __Host-pgs_sid=${sid}" "${origin}${base_path}${probe_path}")" || code=""
  docker logs "$ctl" > "$tmp/ctl.log" 2>&1 || logs_rc=$?
  if [ "$logs_rc" -ne 0 ]; then
    echo "::error::${app}: kernel control row NOT checked — docker logs exited ${logs_rc} on ${ctl}." >&2
    return 1
  fi
  grep -F -q -- "$kernel_line" "$tmp/ctl.log" || g_rc=$?
  case "$code" in
    5??)
      if [ "$g_rc" -eq 0 ]; then
        echo "  ${app}: kernel control: ${base_path}${probe_path} answers ${code} with ${n} corrupted wasm chunk(s), and the log holds '${kernel_line}' — the kernel route row can see a kernel failure"
        return 0
      fi
      if [ "$g_rc" -eq 1 ]; then
        echo "::error::${app}: the kernel control answered ${code} for a reason that is not the kernel — its full log holds no '${kernel_line}'. Check that the control container gets the same arguments as the main one (console_container_args). Its last log lines follow." >&2
      else
        echo "::error::${app}: kernel control row NOT checked — grep exited ${g_rc} on the control log." >&2
      fi
      tail -30 "$tmp/ctl.log" >&2 || true
      return 1
      ;;
    200)
      echo "::error::${app}: the kernel control answered 200 with every wasm chunk corrupted — the kernel route row cannot see a kernel failure. This is the defect SMA-675 closes." >&2
      return 1
      ;;
    *)
      echo "::error::${app}: the kernel control answered '${code:-no response}', not a 5xx. Its last log lines follow." >&2
      tail -30 "$tmp/ctl.log" >&2 || true
      return 1
      ;;
  esac
}
```

Check: `chunk_js` has no line that is exactly `}` (the loader rule in Global Constraints). The lines `};` and `  }` are safe.

- [ ] **Step 4: Run the self-test under both bashes**

Run: `cd <worktree> && /bin/bash ci/images/console-selftest.sh; echo rc=$?` (and with a bash 5).
Expected: `rc=0`; X0 to X10 and every `-rm`, `-argv`, `-create`, `-cp*`, `-seed`, `-healthz`, `-probe` row PASS.

- [ ] **Step 5: Commit**

```bash
cd <worktree> && git branch --show-current && git add ci/images/run.sh ci/images/console-selftest.sh && git commit -m "feat(ci): add the kernel control row with a corrupted wasm chunk (SMA-675)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Wire the rows into `smoke_consoles`, the Q5 switch and the call-site pins

**Files:**
- Modify: `ci/images/run.sh` (`console_probe_path_for` comment; new `kernel_control_flag`; `smoke_consoles`; the `smoke` and `all-consoles` dispatch arms)
- Test: `ci/images/console-selftest.sh`

**Interfaces:**
- Consumes: every function of Tasks 2 to 5; `CONSOLE_SMOKE_REDIS_IMAGE`, `CONSOLE_KERNEL_LINE`, `CONSOLE_SMOKE_NETWORK`.
- Produces:
  - `kernel_control_flag` → prints `--kernel-control=on` (variable unset or `on`) or `--kernel-control=off`; rc 1 with a named `::error::` for any other value, the empty string included.
  - `smoke_consoles --kernel-control=on|off <zone>=<image>...` → a missing or unknown first word is rc 1 with a named `::error::`, before any docker call.

- [ ] **Step 1: Write the failing rows and pins**

Add `kernel_control_flag` to the end of the `FUNCS=` string. Then, directly above `# --- summary ---`, add:

```bash
# --- SMA-675: the Q5 switch (KC, Z, D1 rows) --------------------------------------------------
kc_set() { PAIGASUS_SMOKE_KERNEL_CONTROL="$1"; kernel_control_flag; }
kc_unset() { unset PAIGASUS_SMOKE_KERNEL_CONTROL; kernel_control_flag; }
stub_reset
run_fn KC0 0 "" "::error::" none kc_unset
expect_in KC0-out "$T/KC0.out" "--kernel-control=on"
stub_reset
run_fn KC1 0 "" "::error::" none kc_set off
expect_in KC1-out "$T/KC1.out" "--kernel-control=off"
stub_reset
run_fn KC2 1 "PAIGASUS_SMOKE_KERNEL_CONTROL must be 'on' or 'off'" "" none kc_set bogus
# Review Focus 3: an empty value is a usage error, not a silent default.
stub_reset
run_fn KC3 1 "PAIGASUS_SMOKE_KERNEL_CONTROL must be 'on' or 'off'" "" none kc_set ""

# smoke_case <args...> — smoke_consoles with its globals set to dummy values and every row that
# needs a real image replaced, so only the zone loop's wiring runs. Z_CALLS records which kernel
# rows smoke_consoles called. Z_REDIS_RC is what the Redis start returns.
Z_CALLS="$T/z-calls"
Z_REDIS_RC=0
smoke_case() {
  CONSOLE_SMOKE_ENV=(-e "PAIGASUS_OIDC_ISSUER=https://idp.example.com")
  CONSOLE_HC_DEADLINE=5
  RUN_ID="z"
  CONSOLE_SMOKE_NAMES=""
  CONSOLE_SMOKE_NETWORK=""
  CONSOLE_SMOKE_REDIS_IMAGE="redis:stub"
  CONSOLE_KERNEL_LINE="CompileError"
  CONSOLE_STAGED_TREE_JS=""
  assert_fresh() { return 0; }
  console_node_version_row() { return 0; }
  console_image_config_row() { return 0; }
  console_healthcheck_row() { return 0; }
  console_smoke_redis_start() { return "$Z_REDIS_RC"; }
  console_seed_session() { return 0; }
  console_kernel_route_row() { echo "route $2$3" >> "$Z_CALLS"; }
  console_kernel_control_row() { echo "control $1" >> "$Z_CALLS"; }
  smoke_consoles "$@"
}
# The stub curl answers the zone loop's three requests per zone in order: the page status (200),
# the cookie-less row (302), and the page body (no chunk URL, so the chunk rows red; Z rows do not
# read the rc).
z_curl() {
  curl_reset "$1"
  curl_resp 1 "200" "" 0; curl_resp 2 "302" "" 0; curl_resp 3 "<html></html>" "" 0
  curl_resp 4 "200" "" 0; curl_resp 5 "302" "" 0; curl_resp 6 "<html></html>" "" 0
}

stub_reset; z_curl Z1; rm -f "$Z_CALLS"; Z_REDIS_RC=0
run_fn Z1 1 "" "" any smoke_case --kernel-control=off iam=img:dev gateway=img:dev
expect_in Z1-skip-iam "$T/Z1.out" "iam-console: kernel control row skipped (PAIGASUS_SMOKE_KERNEL_CONTROL=off, the release path; SMA-675 Q5)"
expect_in Z1-skip-gw "$T/Z1.out" "gateway-console: kernel control row skipped (PAIGASUS_SMOKE_KERNEL_CONTROL=off"
expect_in Z1-route-iam "$Z_CALLS" "route /iam/orgs"
expect_in Z1-route-gw "$Z_CALLS" "route /gateway/overview"
expect_not_in Z1-nocontrol "$Z_CALLS" "control"
expect_no_call Z1 "-nokernel-"
expect_call Z1-net "--network smoke-net-z"
expect_call Z1-redis "PAIGASUS_SESSION_REDIS_URL=redis://smoke-redis-z:6379"
stub_reset; z_curl Z2; rm -f "$Z_CALLS"; Z_REDIS_RC=0
run_fn Z2 1 "" "" any smoke_case --kernel-control=on iam=img:dev gateway=img:dev
expect_in Z2-control-iam "$Z_CALLS" "control iam-console"
expect_in Z2-control-gw "$Z_CALLS" "control gateway-console"
expect_not_in Z2-noskip "$T/Z2.out" "kernel control row skipped"
stub_reset
run_fn Z3 1 "the first argument must be --kernel-control=on or --kernel-control=off, not 'iam=img:dev'" "" none smoke_case iam=img:dev
stub_reset
run_fn Z4 1 "not '--kernel-control=maybe'" "" none smoke_case --kernel-control=maybe iam=img:dev
# D6: with no Redis, the kernel rows do not run and the containers get the memory store.
stub_reset; z_curl Z5; rm -f "$Z_CALLS"; Z_REDIS_RC=1
run_fn Z5 1 "iam-console: kernel rows NOT run" "" any smoke_case --kernel-control=on iam=img:dev gateway=img:dev
expect_not_in Z5-norows "$Z_CALLS" "route"
expect_call Z5-mem "PAIGASUS_SESSION_STORE=memory"
expect_no_call Z5 "--network"
Z_REDIS_RC=0

# D1: the dispatch arm, through the REAL script. It must stop before any docker call.
rm -f "$T/argv"
D1_RC=0
( PATH="$T/stub:$PATH"; STUB_ARGV="$T/argv"; export PATH STUB_ARGV
  PAIGASUS_SMOKE_KERNEL_CONTROL=bogus "$BASH" "$RUN_SH" all-consoles ) >"$T/D1.out" 2>"$T/D1.err" || D1_RC=$?
check_row D1 "$D1_RC" 1 "PAIGASUS_SMOKE_KERNEL_CONTROL must be 'on' or 'off'" "" "$T/D1.out" "$T/D1.err"
if [ -e "$T/argv" ]; then say_fail D1-nodocker "the stub docker was called" "$T/argv"; else say_pass D1-nodocker; fi

# --- SMA-675 call-site pins (P3 to P16) --------------------------------------------------------
# shellcheck disable=SC2016 # the pinned lines are literal text
pin_rows P3 "$T/fn-smoke_consoles.sh" 'if console_smoke_redis_start "$CONSOLE_SMOKE_NETWORK" "$redis_name" "$CONSOLE_SMOKE_REDIS_IMAGE" 20; then kernel_ok=1; else ec=1; fi'
# shellcheck disable=SC2016
pin_rows P4 "$T/fn-smoke_consoles.sh" 'console_container_args "$app" "$service" "$zones_json" "$net" "$redis_url" "$work/env" > "$args_file" || args_rc=$?'
# shellcheck disable=SC2016
pin_rows P5 "$T/fn-smoke_consoles.sh" 'run_out="$(docker create --name "$name" "${cargs[@]}" "$image" 2>&1)" || run_rc=$?'
# shellcheck disable=SC2016
pin_rows P6 "$T/fn-smoke_consoles.sh" 'elif console_seed_session "$redis_name" "$sid"; then'
# shellcheck disable=SC2016
pin_rows P7 "$T/fn-smoke_consoles.sh" 'console_kernel_route_row "$origin" "$base_path" "$console_path" "$sid" "$name" 3 || ec=1'
# shellcheck disable=SC2016
pin_rows P8 "$T/fn-smoke_consoles.sh" 'console_kernel_control_row "$app" "$image" "$base_path" "$console_path" "$redis_name" "$args_file" "$CONSOLE_KERNEL_LINE" || ec=1'
# shellcheck disable=SC2016
pin_rows P9 "$T/fn-console_kernel_control_row.sh" 'rm_out="$(docker rm -f "$ctl" 2>&1)" || rm_rc=$?'
# shellcheck disable=SC2016
pin_rows P10 "$T/fn-console_kernel_control_row.sh" 'console_kernel_control_probe "$ctl" "$tmp" "$@" || rc=$?'
# shellcheck disable=SC2016
pin_rows P11 "$T/fn-console_kernel_route_row.sh" 'grep -F -q -- "$nonce" "$body" || g_rc=$?'
# shellcheck disable=SC2016
pin_rows P12 "$T/fn-console_kernel_control_probe.sh" 'grep -F -q -- "$kernel_line" "$tmp/ctl.log" || g_rc=$?'
# shellcheck disable=SC2016
pin_rows P13 "$T/fn-console_kernel_control_probe.sh" 'out="$(docker create --name "$ctl" "${cargs[@]}" "$image" 2>&1)" || rc=$?'
# shellcheck disable=SC2016
pin_rows P14 "$T/fn-console_kernel_control_probe.sh" 'if ! console_seed_session "$redis" "$sid"; then'
# shellcheck disable=SC2016
pin_rows P15 "$T/fn-console_smoke_redis_start.sh" 'out="$(docker network create --label paigasus.smoke=console "$network" 2>&1)" || rc=$?'
# shellcheck disable=SC2016
pin_rows P16 "$T/fn-console_smoke_cleanup.sh" 'docker network rm "$CONSOLE_SMOKE_NETWORK" >/dev/null 2>&1 || true'
```

- [ ] **Step 2: Run the self-test and see it fail**

Run: `cd <worktree> && /bin/bash ci/images/console-selftest.sh; echo rc=$?`
Expected: `no 'kernel_control_flag() {' line`, `rc=2`.

- [ ] **Step 3: Add `kernel_control_flag`**

Directly after the closing `}` of `console_kernel_control_probe`, add:

```bash

# SMA-675 Q5: the kernel control row runs only on the images.yml paths. release.yml's "Smoke this
# service" step sets PAIGASUS_SMOKE_KERNEL_CONTROL to off. An UNSET variable means on, so a
# missing setting runs more checks, not fewer. Any other value, the empty string included, is a
# usage error. It prints the first argument of smoke_consoles. The dispatch arms call it; no row
# function reads the variable.
kernel_control_flag() {
  case "${PAIGASUS_SMOKE_KERNEL_CONTROL-on}" in
    on) echo "--kernel-control=on" ;;
    off) echo "--kernel-control=off" ;;
    *)
      echo "::error::PAIGASUS_SMOKE_KERNEL_CONTROL must be 'on' or 'off' (unset means on), not '${PAIGASUS_SMOKE_KERNEL_CONTROL-}'." >&2
      return 1
      ;;
  esac
}
```

- [ ] **Step 4: Rewrite the comment above `console_probe_path_for` (D5)**

Replace the first paragraph of that comment (run.sh:589-591, from `# A route INSIDE` to `# @paigasus/console-core, which evaluates the kernel's wasm at module scope.`) with:

```bash
# A route INSIDE that zone's `(console)` route group, relative to its basePath (SMA-634). Two rows
# of smoke_consoles use it. The cookie-less row proves only that proxy.ts gates the zone: the
# proxy redirects a request with no session cookie before Next routes it, so that row loads no
# kernel code. The kernel route row (console_kernel_route_row, SMA-675) sends a seeded session.
# Then the `(console)` layout runs, and it imports @paigasus/console-core, which evaluates the
# kernel's wasm at module scope. The kernel control row proves that the route row reds when the
# wasm cannot load.
```

Keep the `PER ZONE` paragraph below it unchanged.

- [ ] **Step 5: Change the start of `smoke_consoles`**

Replace the `local` lines at the top of `smoke_consoles` (run.sh, the three `local` lines and `local ec=0 bad started`) with:

```bash
  local spec image service app base_path console_path other name port origin status html chunk bytes code uid console_status
  local run_out img_out img_public img_list host_public host_list img_dirs host_dirs
  local host_std host_static host_id run_rc sh_rc img_rc cstate
  local kernel_control kernel_ok redis_name work zones_json net redis_url args_file args_rc line sid cargs
  local ec=0 bad started
  # SMA-675 Q5: the first word says whether the kernel control row runs. It is an argument, not a
  # global (the SMA-670 rule); the dispatch arms derive it with kernel_control_flag. It is read
  # before the trap, so a usage error makes no docker call.
  case "${1:-}" in
    --kernel-control=on) kernel_control="on" ;;
    --kernel-control=off) kernel_control="off" ;;
    *)
      echo "::error::smoke_consoles: the first argument must be --kernel-control=on or --kernel-control=off, not '${1:-<none>}'." >&2
      return 1
      ;;
  esac
  shift
```

Then, directly after the existing `if [ "$#" -eq 0 ]; then … fi` block (the "called with no zones" check), add:

```bash

  # SMA-675: the fixed env words go to a file, so console_container_args reads no global. The
  # PAIGASUS_ZONES JSON is the same hardcoded two-zone assumption that the `other` prefix below
  # names.
  work="$(mktemp -d "${TMPDIR:-/tmp}/paigasus-console-smoke.XXXXXX")" || work=""
  if [ -z "$work" ]; then
    echo "::error::smoke_consoles: mktemp failed — nothing was smoked." >&2
    return 1
  fi
  printf '%s\n' "${CONSOLE_SMOKE_ENV[@]}" > "$work/env"
  zones_json='{"iam":"/iam","gateway":"/gateway"}'

  # SMA-675 D6 and D7: one network and one Redis sidecar per run, shared by both zones, as in
  # production. Both names are registered BEFORE they are created, so the EXIT trap removes them
  # after an abort. A failure skips only the kernel rows: the containers then start on the old
  # memory store with no network, and every other row still runs and reports.
  redis_name="smoke-redis-${RUN_ID}"
  CONSOLE_SMOKE_NAMES="$CONSOLE_SMOKE_NAMES $redis_name"
  CONSOLE_SMOKE_NETWORK="smoke-net-${RUN_ID}"
  kernel_ok=0
  if console_smoke_redis_start "$CONSOLE_SMOKE_NETWORK" "$redis_name" "$CONSOLE_SMOKE_REDIS_IMAGE" 20; then kernel_ok=1; else ec=1; fi
```

- [ ] **Step 6: Replace the `docker run -d` of the main container**

In the comment above `if [ "$service" = "iam" ]; then other="/gateway"; …`, replace the sentence `The PAIGASUS_ZONES JSON literal in the \`docker run\` below is a third hardcoded copy of the same two-zone assumption.` with `The zones_json literal above is a third hardcoded copy of the same two-zone assumption.`

Replace the block from `run_rc=0` through the closing `fi` of `if [ "$run_rc" -ne 0 ]; then … else started=1; fi` (run.sh:1464-1476), and the comment lines directly above it that start with `# -p 0:3000 asks the daemon`, with:

```bash
    # SMA-675 D4 step 2: the main container gets its arguments from console_container_args, the
    # same function as the kernel control container. `docker create` + `docker start`, not
    # `docker run -d`, so the two containers are made the same way. -p 0:3000 (in the list) asks
    # the daemon for a free ephemeral port. GUARDED, and docker's own message is printed on its
    # own lines, because a `::error::` line with an embedded newline stops being one annotation.
    if [ "$kernel_ok" -eq 1 ]; then
      net="$CONSOLE_SMOKE_NETWORK"
      redis_url="redis://${redis_name}:6379"
    else
      net=""
      redis_url=""
      echo "::error::${app}: kernel rows NOT run — the Redis sidecar is not available this run (see the error above); this container uses the memory store." >&2
    fi
    args_file="$work/args-${service}"
    args_rc=0
    console_container_args "$app" "$service" "$zones_json" "$net" "$redis_url" "$work/env" > "$args_file" || args_rc=$?
    if [ "$args_rc" -ne 0 ]; then
      ec=1; bad=1
    else
      cargs=()
      while IFS= read -r line; do
        cargs[${#cargs[@]}]="$line"
      done < "$args_file"
      run_rc=0
      run_out="$(docker create --name "$name" "${cargs[@]}" "$image" 2>&1)" || run_rc=$?
      if [ "$run_rc" -eq 0 ]; then
        run_out="$(docker start "$name" 2>&1)" || run_rc=$?
      fi
      if [ "$run_rc" -ne 0 ]; then
        echo "::error::${app}: the container did not start from ${image} — docker exited ${run_rc}; its own message follows. If the image is missing, build it first: 'ci/images/run.sh build-console ${service}', or 'build-oci' and 'load-oci' for the release archive." >&2
        printf '%s\n' "$run_out" >&2
        ec=1; bad=1
      else
        started=1
      fi
    fi
```

The existing lines `echo "== smoke ${app} =="`, `bad=0` and `started=0` stay directly above this block.

- [ ] **Step 7: Rewrite the cookie-less row and add the kernel rows (D5)**

Replace the whole comment block that starts with `# SMA-634. A (console) route, which imports @paigasus/console-core` and ends with `# and to what this suite deploys, not a change to this probe.` (run.sh:1542-1572) with:

```bash
    # SMA-634, corrected by SMA-675 D5. The cookie-less row: a request with NO session cookie to
    # the zone's (console) route. proxy.ts answers it with a 3xx on cookie PRESENCE alone
    # (ADR-0017 decision 7), before Next routes it. So this row proves only that the proxy gate is
    # in effect. It loads no kernel code: a path that does not exist answers 307 too (measured,
    # SMA-634). The kernel proof is the kernel route row and its control, directly below.
    # Deliberately no `-L`: the redirect is the assertion, and following it would reach the IdP.
```

In the `case "$console_status" in` below it, replace the `3??)` line and the `::error::` line with:

```bash
        3??) echo "  ${app}: ${base_path}${console_path} redirects (${console_status}) with no session cookie — the proxy gate is in effect (the kernel rows below prove the kernel load)" ;;
```

```bash
          echo "::error::${app}: ${base_path}${console_path} answered '${console_status:-no response}' to a request with no session cookie — proxy.ts must redirect it (3xx), so the proxy gate is not in effect. Read 'docker logs ${name}'." >&2
```

Directly after the `fi` that closes this cookie-less row (before `html=""`), add:

```bash

    # SMA-675 D3 and D4: the kernel rows. The route row sends a seeded session (a new sid per
    # container, D7) to the same route, and passes only on 200 with the session's nonce. The
    # control row starts a second container from the same image and the same argument list, with
    # every wasm chunk corrupted, and passes only on a 5xx with the kernel line in its log. It runs
    # only on the images.yml paths (Q5).
    if [ "$kernel_ok" -eq 1 ] && [ "$bad" -eq 0 ]; then
      sid="$(console_new_sid)" || sid=""
      if [ -z "$sid" ]; then
        echo "::error::${app}: kernel route row NOT run — no session id could be read from /dev/urandom." >&2
        ec=1
      elif console_seed_session "$redis_name" "$sid"; then
        console_kernel_route_row "$origin" "$base_path" "$console_path" "$sid" "$name" 3 || ec=1
      else
        ec=1
      fi
    fi
    if [ "$kernel_ok" -eq 1 ] && [ "$args_rc" -eq 0 ]; then
      if [ "$kernel_control" = "on" ]; then
        console_kernel_control_row "$app" "$image" "$base_path" "$console_path" "$redis_name" "$args_file" "$CONSOLE_KERNEL_LINE" || ec=1
      else
        echo "  ${app}: kernel control row skipped (PAIGASUS_SMOKE_KERNEL_CONTROL=off, the release path; SMA-675 Q5)"
      fi
    fi
```

- [ ] **Step 7b: Remove the memory store from `CONSOLE_SMOKE_ENV` (AC 3)**

In `run.sh`, delete the line `  -e "PAIGASUS_SESSION_STORE=memory"` from the `CONSOLE_SMOKE_ENV=(…)` array. Directly above the `CONSOLE_SMOKE_ENV=(` line, add:

```bash
# SMA-675: the array holds only the values that do not depend on the run. The session store env
# (redis and its URL, or the memory store in the D6 fallback) comes from console_container_args.
```

Add this row to the self-test, directly after the Z rows:

```bash
# AC 3: CONSOLE_SMOKE_ENV sets no session store any more; console_container_args owns it.
awk '/^CONSOLE_SMOKE_ENV=\($/ { on = 1 } on { print } on && /^\)$/ { on = 0 }' "$RUN_SH" > "$T/env-block"
if [ -s "$T/env-block" ]; then say_pass ENV0-found; else say_fail ENV0-found "no CONSOLE_SMOKE_ENV=( block in run.sh"; fi
expect_not_in ENV0 "$T/env-block" "PAIGASUS_SESSION_STORE"
```

Also add to the Z1 checks:

```bash
expect_no_call Z1-nomem "PAIGASUS_SESSION_STORE=memory"
```

- [ ] **Step 8: Remove the work directory at the end**

Replace the line `  console_smoke_cleanup` directly after the `done` of the zone loop (run.sh:1787) with:

```bash
  rm -rf "$work"
  console_smoke_cleanup
```

- [ ] **Step 9: Pass the flag from both dispatch arms**

In the `smoke)` arm, replace:

```bash
    if [ "$smoke_kind" = cargo ]; then smoke "$@"; exit 0; fi
    smoke_keys=("$@")
```

with:

```bash
    if [ "$smoke_kind" = cargo ]; then smoke "$@"; exit 0; fi
    # SMA-675 Q5: only the console branch reads the switch; a cargo smoke ignores it.
    kc_flag="$(kernel_control_flag)" || exit 1
    smoke_keys=("$@")
```

and replace the arm's last `smoke_consoles "$@"` with `smoke_consoles "$kc_flag" "$@"`.

In the `all-consoles)` arm, directly after its `if [ -n "$target" ]; then … fi` block and BEFORE `assert_console_pins`, add:

```bash
    # SMA-675 Q5: read first, so a bad value stops before the build.
    kc_flag="$(kernel_control_flag)" || exit 1
```

and replace the arm's last `smoke_consoles "$@"` with `smoke_consoles "$kc_flag" "$@"`.

- [ ] **Step 10: Run the self-test under both bashes**

Run: `cd <worktree> && /bin/bash ci/images/console-selftest.sh; echo rc=$?` (and with a bash 5).
Expected: `rc=0`. KC0 to KC3, Z1 to Z5 and every sub-row, ENV0, D1, D1-nodocker, P3 to P16 and every `-mut` row PASS. The existing P1a, P1b, P1c and P2 rows still PASS. `bash -n ci/images/run.sh` prints nothing.

- [ ] **Step 11: Commit**

```bash
cd <worktree> && git branch --show-current && git add ci/images/run.sh ci/images/console-selftest.sh && git commit -m "feat(ci): run the kernel rows in the console smoke, with a release-path switch (SMA-675)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Workflows, the `SessionRecord` pointer and the RUNBOOK

**Files:**
- Modify: `.github/workflows/release.yml:1245-1246`
- Modify: `.github/workflows/images.yml` (the `pull_request.paths` list)
- Modify: `ts/packages/paigasus-auth/src/core/session.ts` (the `version` doc comment)
- Modify: `docs/ops/RUNBOOK-containers.md` (lines 33-43, 45-62 and after the zone-row bullet near line 438)

**Interfaces:**
- Consumes: the names `PAIGASUS_SMOKE_KERNEL_CONTROL`, `console_seed_session`, `CONSOLE_SMOKE_REDIS_IMAGE` (Tasks 2, 3, 6).
- Produces: documentation and CI triggers only.

- [ ] **Step 1: Turn the control off on the release path**

In `.github/workflows/release.yml`, replace:

```yaml
      - name: Smoke this service
        run: ci/images/run.sh smoke "${SERVICE}"
```

with:

```yaml
      - name: Smoke this service
        # SMA-675 Q5: the kernel control row (a second container with a corrupted wasm chunk)
        # runs only in images.yml. It tests the probe, not the release candidate. The kernel route
        # row and the Redis sidecar still run here, on amd64 and arm64. A cargo service smoke
        # ignores the variable. The value is quoted: YAML 1.1 reads a bare off as a boolean.
        env:
          PAIGASUS_SMOKE_KERNEL_CONTROL: 'off'
        run: ci/images/run.sh smoke "${SERVICE}"
```

This step is inside the `&image-build-steps` anchor, so all four image-build jobs get it.

- [ ] **Step 2: Verify the value is the string `off` (Review Focus 4)**

```bash
cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && PROTO_REPORTER=text uv run --locked --project py python -c '
import yaml
d = yaml.safe_load(open(".github/workflows/release.yml"))
vals = [s["env"]["PAIGASUS_SMOKE_KERNEL_CONTROL"] for j in d["jobs"].values() for s in j.get("steps", []) if s.get("name") == "Smoke this service"]
print(vals)
assert vals and all(v == "off" for v in vals), vals
'
```

Expected: `['off', 'off', 'off', 'off']` and exit 0.

- [ ] **Step 3: Add the four coupled paths to `images.yml`**

In `.github/workflows/images.yml`, directly after the line `      - 'rs/crates/bindings/paigasus-node-bindings/index.d.ts'`, add:

```yaml
      # SMA-675: the console smoke's kernel route row depends on these four files at RUNTIME,
      # not at build time. It seeds a literal SessionRecord, which isSessionRecord in session.ts
      # must accept. It expects the seeded session's nonce from the (console) layout on each
      # zone's probe page (console_probe_path_for in ci/images/run.sh). A change here can red the
      # smoke while the ordinary build stays green.
      - 'ts/packages/paigasus-auth/src/core/session.ts'
      - 'ts/apps/*/app/(console)/layout.tsx'
      - 'ts/apps/iam-console/app/(console)/orgs/page.tsx'
      - 'ts/apps/gateway-console/app/(console)/overview/page.tsx'
```

GitHub path filters treat `(` and `)` as literal characters; the quotes keep YAML from reading them.

- [ ] **Step 4: Add the pointer line to `SessionRecord.version`**

In `ts/packages/paigasus-auth/src/core/session.ts`, in the doc comment directly above `  version: 2;`, replace the line `   * (spec § 4.1).` with:

```ts
   * (spec § 4.1).
   * The console image smoke seeds a literal record of this version (`ci/images/run.sh`,
   * `console_seed_session`); change it too.
```

- [ ] **Step 5: Update the RUNBOOK path-filter list (lines 33-43)**

In `docs/ops/RUNBOOK-containers.md`, in the parenthesised filter list, replace:

```text
`rs/crates/bindings/paigasus-node-bindings/index.d.ts`,
`ci/images/**`,
```

with:

```text
`rs/crates/bindings/paigasus-node-bindings/index.d.ts`,
`ts/packages/paigasus-auth/src/core/session.ts`, `ts/apps/*/app/(console)/layout.tsx`,
`ts/apps/iam-console/app/(console)/orgs/page.tsx`,
`ts/apps/gateway-console/app/(console)/overview/page.tsx`,
`ci/images/**`,
```

- [ ] **Step 6: Add the second clause to the filter rule (after line 62)**

Directly after the paragraph that ends with `…and \`ts/Dockerfile\` copies both by name, so they are on the filter.`, add a new paragraph:

```markdown
A second clause covers the image SMOKE (SMA-675). A file is also on the filter when the console
smoke depends on it at runtime, so that a change to it can red the smoke while the ordinary build
stays green. Four files are on the filter for this reason:
`ts/packages/paigasus-auth/src/core/session.ts` (the smoke seeds a literal `SessionRecord`, which
`isSessionRecord` must accept), the two `ts/apps/*/app/(console)/layout.tsx` files (they put the
seeded session's nonce into the page), and the two probe pages
`ts/apps/iam-console/app/(console)/orgs/page.tsx` and
`ts/apps/gateway-console/app/(console)/overview/page.tsx`. A new probe path in
`console_probe_path_for` must add its page to the filter. Nothing checks this.
```

- [ ] **Step 7: Add the kernel-row bullet after the zone-row bullet (near line 438)**

Directly after the bullet that starts `- **The zone row of \`smoke_consoles\` proves only that a basePath is in effect.**` and ends `…both zones hydrate through one Traefik ingress.`, add:

```markdown
- **The kernel rows of `smoke_consoles` prove that the image loads the kernel (SMA-675).**
  - The kernel route row seeds a session record in Redis and sends its cookie to the zone's
    `(console)` route (`/iam/orgs`, `/gateway/overview`). The `(console)` layout imports the
    kernel at module scope. The row passes only on 200 with a per-run nonce from the seeded
    record in the body. The brand label is not a marker: every page holds it in `<title>`.
  - The kernel control row starts a second container from the same image and the same arguments,
    with every `*paigasus_wasm_bg*.wasm` chunk corrupted. It passes only when the same request
    answers 5xx and the container log holds the kernel compile error. So a 200 from the route row
    cannot come from a path that skips the kernel.
  - The rows do NOT prove that IAM calls work (the smoke has no IAM), that a real login works (the
    kind e2e tier covers that), or that a kernel function returns correct values (the kernel tests
    cover that).
  - The cookie-less row proves only that the proxy redirects a request with no session cookie. It
    loads no kernel code.
  - The smoke starts one Redis container from a pinned digest, on a per-run Docker network. The
    discovery descriptor cache of the consoles uses that Redis too.
  - The console release chain in `release.yml` now pulls `redis:7.4-alpine` from Docker Hub, on
    amd64 and arm64. A Docker Hub outage or rate limit can red a console release.
  - The control row runs only on the `images.yml` paths. `release.yml` sets
    `PAIGASUS_SMOKE_KERNEL_CONTROL: 'off'` on "Smoke this service". Unset or `on` runs the
    control; any other value is a usage error. The smoke removes the control container as soon as
    its row finishes.
  - The Redis digest has two copies: `CONSOLE_SMOKE_REDIS_IMAGE` in `ci/images/run.sh` and
    `ci/kind/manifests/redis.yaml`. Each names the other. No check compares them.
  - A killed run can leave the Redis container and the network behind. Both carry the label
    `paigasus.smoke=console`. Remove them with
    `docker rm -f $(docker ps -aq --filter label=paigasus.smoke=console)`, then
    `docker network prune --filter label=paigasus.smoke=console`.
```

- [ ] **Step 8: Run the checks for these files**

```bash
cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && git add -A .github docs ts/packages/paigasus-auth/src/core/session.ts && moon run paigasus-auth-ts:lint ts:fmt
```

Expected: both tasks pass. (If the project id differs, find it with `moon query projects | grep -n paigasus-auth`.)

Then run the workflow gate (it needs a bash 5 and a healthy pipe; read its preflight line):

```bash
cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && /opt/homebrew/bin/bash ci/actionlint/run.sh; echo rc=$?
```

Expected: `rc=0`. If the preflight reports `pipe capacity 512 bytes`, there is no local verdict (root CLAUDE.md, SMA-612): record it, and rely on CI's `repo:actionlint`. Do not read that rc 2 as a finding.

- [ ] **Step 9: Commit**

```bash
cd <worktree> && git branch --show-current && git add .github/workflows/release.yml .github/workflows/images.yml ts/packages/paigasus-auth/src/core/session.ts docs/ops/RUNBOOK-containers.md && git commit -m "feat(ci): run the kernel smoke on session and probe changes, and skip its control on release (SMA-675)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Real smoke, delete-the-feature proof and the full gate graph

This task adds no feature code. Its deliverables are a green real smoke, the recorded result of each spec §9 mutation, and a green gate graph. Keep the evidence in `$S/sma-675-evidence.md` for the PR description.

**Files:** temporary edits only, each reverted with an Edit (NEVER with `git checkout --`, `git restore` or `git stash`, so the committed work is safe).

**Interfaces:**
- Consumes: everything above.
- Produces: the evidence for AC 1 to AC 8.

- [ ] **Step 1: Run the real smoke**

```bash
cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && /bin/bash ci/images/run.sh all-consoles 2>&1 | tee "$S/smoke-clean.log"; echo "rc=${PIPESTATUS[0]}"
```

Expected: `rc=0`, `== CONSOLE SMOKE OK ==`, and for each zone: `Redis sidecar … answers PONG`, `redirects (…) with no session cookie — the proxy gate is in effect`, `kernel row /iam/orgs: 200 with the seeded session's nonce` (or the D3a OK line), and `kernel control: … answers 5xx … and the log holds 'CompileError'`. After the run, `docker ps -a --filter label=paigasus.smoke=console` and `docker ps -a | grep nokernel` print nothing, and `docker network ls | grep smoke-net` prints nothing.

- [ ] **Step 2: Run the release-path switch (mutation 10)**

```bash
cd <worktree> && PAIGASUS_SMOKE_KERNEL_CONTROL=off /bin/bash ci/images/run.sh all-consoles 2>&1 | tee "$S/smoke-off.log"; echo "rc=${PIPESTATUS[0]}"
```

In a second shell during the run, poll `docker ps -a --format '{{.Names}}' | grep -c nokernel` (expected 0 every time). Expected: `rc=0`, one `kernel control row skipped` line per zone, a green route row per zone. Then run once with `PAIGASUS_SMOKE_KERNEL_CONTROL=bogus` and expect rc 1 with the usage message and no build.

- [ ] **Step 3: Run mutations 1 to 7 and 9 from spec §9**

For each mutation: make the one edit with the Edit tool, run the command, record the named message and the rc in `$S/sma-675-evidence.md`, then revert the edit with the Edit tool (the inverse edit), and check that `git diff --stat` shows no change in `ci/images/run.sh`. Each of mutations 1 to 7 needs one full `/bin/bash ci/images/run.sh all-consoles` run (it rebuilds both images; the Docker layer cache makes the rebuild fast).

| # | Edit | Expected message |
|---|------|------------------|
| 1 | In `console_kernel_route_row`, in the curl call, change `-H "Cookie: __Host-pgs_sid=${sid}" "$url")"` to `"$url")"` | `the proxy did not see the session cookie` (not the requireSession message) |
| 2 | In `console_seed_session`, change `\"version\":2` to `\"version\":3` | `requireSession found no session after 3 requests` |
| 3 | In `console_container_args`, change `-e "PAIGASUS_SESSION_STORE=redis"` to `-e "PAIGASUS_SESSION_STORE=memory"` | `kernel row /iam/orgs: answered 500` |
| 4 | In `console_kernel_control_probe`, make the overwrite a no-op: change `"${ctl}:${path}"` in the `docker cp` line to `"${ctl}:${path}.unused"` (the real chunk stays intact) | `the kernel control answered 200 with every wasm chunk corrupted` |
| 5 | In `console_kernel_control_probe`, change the create line to `out="$(docker create --name "$ctl" "${cargs[@]}" -e PAIGASUS_SESSION_REDIS_URL= "$image" 2>&1)" \|\| rc=$?` (docker keeps the LAST value of a repeated `-e`) | `the kernel control answered 500 for a reason that is not the kernel`. If Task 1 recorded that the kernel line appears at START (preload), this mutation can PASS, because the compile error is in the log anyway. Record that result as a finding for the PR; do not hide it. |
| 6 | In `console_kernel_route_row`, change `nonce="smoke-${sid:0:12}@example.com"` to `nonce="smoke-${sid:1:12}@example.com"` | `answered 200, but the body holds no smoke-` |
| 7 | In `console_probe_path_for`, change `iam)     echo "/orgs" ;;` to `iam)     echo "/no-such-route" ;;` | `kernel row /iam/no-such-route: answered 404` (the cookie-less row stays green: the proxy answers 307) |
| 9 | Delete the line `  rm_out="$(docker rm -f "$ctl" 2>&1)" \|\| rm_rc=$?` in `console_kernel_control_row` | run the SELF-TEST: `P9` FAILs and every `X*-rm` row FAILs |

Mutation 8: delete the line `        console_kernel_route_row "$origin" "$base_path" "$console_path" "$sid" "$name" 3 || ec=1` from `smoke_consoles`, run `/bin/bash ci/images/console-selftest.sh`, expect `FAIL P7`, then revert.

After all mutations, `git diff` must be empty and `/bin/bash ci/images/console-selftest.sh` must give `rc=0` again.

- [ ] **Step 4: Run the self-test under both bashes one last time**

```bash
cd <worktree> && /bin/bash ci/images/console-selftest.sh > "$S/selftest-32.log" 2>&1; echo rc=$?; /opt/homebrew/bin/bash ci/images/console-selftest.sh > "$S/selftest-5.log" 2>&1; echo rc=$?
```

Expected: `rc=0` twice, and the summary line `console-selftest: N passed, 0 failed, 0 skipped` in each (F0/F1 need Docker). If the bash 5 run hangs at about 0% CPU, the host pipe is in the 512-byte state: record it (CI runs Linux bash 5).

- [ ] **Step 5: Run the full gate graph**

```bash
cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && git fetch origin main && moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site :http-extractor-envelope :input-liveness :promtool :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci :next-public-free :helm-render :test-e2e --base origin/main --include-relations
```

Expected: every task passes. Read the root CLAUDE.md "This development Mac only" entry before you trust a red on `repo:actionlint`, `repo:affected-smoke`, `repo:ruff-ci`, `repo:next-public-free` or `repo:publish-metadata`: re-run those gates directly with the bash they need, and use those verdicts.

- [ ] **Step 6: Write the PR evidence notes**

In `$S/sma-675-evidence.md` (the PR description is written from it at the open-PR stage), record:
- M1 to M8 from Task 1, and the row form (`D3` or `D3a`) with the reason (AC 7).
- The clean smoke result and the switch result (Steps 1, 2).
- Each mutation, its message and its rc (Step 3).
- The self-test counts under both bashes (Step 4).
- A reminder for the PR stage: AC 8 needs a green `images.yml` on the PR (amd64) and a `gh workflow run images.yml --ref feature/sma-675-console-smoke-kernel-probe` run for arm64; record that run's URL in the PR.
- The one design choice this plan made that the spec left open: `console_kernel_control_row` writes `CONSOLE_SMOKE_NAMES` itself, and the control work is in a second function `console_kernel_control_probe`, so that one place removes the container on every path.

No commit in this task unless a mutation found a defect. A defect fix goes in its own commit with a test row that reds before the fix, and Steps 1 to 5 run again.
