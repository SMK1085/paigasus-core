# SMA-701 Kind IAM Service Settle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The kind job's `install a` and `upgrade b` steps return only after IAM answers HTTP 200 on
`/readyz` through its Service, as seen from each console pod. A failure gives rc 1 or rc 2 in the
existing classes, and each run prints how long the wait took.

**Architecture:** One new bash helper, `settle_iam_service`, in `ci/kind/run.sh`. Part A runs
`kubectl rollout status` on the IAM Deployment (240 s). Part B runs `node -e "$PROBE_JS"` in each
Running console pod through `kubectl exec`, with a 60 s deadline per pod. A second new helper,
`run_bounded`, stops each `kubectl exec` after 20 s with a watchdog, because macOS has no
`timeout(1)`. `install_a` and `upgrade_b` call the helper. `diagnose` also collects EndpointSlices.
`chart.yml` gets larger step and job timeouts. The README and the `run.sh` header describe the wait.

**Tech Stack:** bash (3.2.57 and 5), kubectl 1.31.14 in CI, Helm 3.22.0, Node 24 in the
distroless console image, GitHub Actions, the pinned shellcheck 0.11.0 (`shellcheck-py` in
`py/uv.lock`).

**Spec:** docs/superpowers/specs/2026-10-02-sma-701-kind-iam-service-settle-design.md

## Global Constraints

- Every line of new bash runs under `/bin/bash` 3.2.57 and under bash 5.
- Do not use `mapfile`, `declare -A`, a here-string (`<<<`) or a here-doc (`<<`) in `ci/kind/run.sh`.
- Do not pipe into an early-exit reader (`grep -q`, `grep -m`, `head`, `awk … exit`, `sed … q`) in `ci/kind/run.sh` (actionlint check 13).
- Every wait has an explicit bound. Do not use `timeout(1)`.
- Part A: `rollout status --timeout=240s`. Part B: a 60 s deadline per pod, a 20 s bound per `kubectl exec`, `sleep 2` between tries.
- `PROBE_JS` uses `AbortSignal.timeout(5000)`.
- `chart.yml`: the `install a` step is 19 min, the `upgrade b` step is 23 min, the job is 182 min. The step sum is 171.
- Failure classes: only the six rows of spec §5. rc 1 is `die_assert`, rc 2 is `die_infra`.
- Do not change the Helm chart (`charts/**`). Do not change the Playwright specs. Do not change `probeService` or any file under `ts/`.
- Do not change the spec file. Stage only the files that a task names.
- Every new source file and every edited file keeps its SPDX header line. No task makes a new tracked file except the plan.
- Commits: conventional, scope `ci`, end with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- A commit body must not hold a line that starts with `#` followed by a number, or a line of the form `token: value`. commitlint reads such a line as a footer and fails `footer-leading-blank`.
- Never use `git commit --amend`, `git reset`, `git stash`, `--no-verify` or `--no-gpg-sign`. If signing fails with "failed to fill whole buffer", 1Password is locked: stop and report.
- Do not install host software (no `brew install`). Do not push, and do not start a CI run, except in Tasks 5 and 6, which the coordinator runs.
- Tool PATH for every shell: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text`.
- `SCRATCH` in the commands below means the scratchpad directory from your system prompt. Run `export SCRATCH=<that directory>` in each shell first. Proof scripts go there, never into the repo.
- The Bash tool runs zsh. The commands below avoid bash-only syntax such as `PIPESTATUS`. The proof scripts run under an explicit `/bin/bash` or `/opt/homebrew/bin/bash`.
- In Tasks 5 and 6, `<id>` is the `databaseId` that `gh run list` prints for the run you started, and `<N>` is the run number 1-5.
- If the worktree sandbox refuses a compound command, write the commands into a script file in `$SCRATCH` and run `/bin/bash "$SCRATCH/<file>"`.
- Do not write the string that names moon's CI report file (`ci` + `Report.json`) into any tracked file. `repo:actionlint` check 12 then needs a marker.

## Review Focus

These five failure modes are the most probable. The offline proofs use a fake `kubectl`, so they
cannot show them. Each line names the check that the owning task adds.

1. **The real kubectl reads the label keys and the selectors differently.** The fake `kubectl` in
   Task 2 ignores the jsonpath, the `in (…)` selector and `--field-selector`. A wrong escape of
   `app\.kubernetes\.io/name` prints empty fields. Check: Task 2 Step 6 runs the two jsonpath
   templates with a real kubectl against a local List file. Task 6 Step 4 asserts the line
   `console Deployments: gateway-console iam-console` in `install a`.
2. **Part B checks zero pods and passes.** An empty jsonpath result or a wrong label would skip
   every pod. Check: the `iam-console` floor and the coverage loop in Task 2. The harness rows S2,
   S4 and S5, and the mutation MA in Task 2 Step 5. Task 6 Step 4 asserts two `reaches IAM` lines
   in `install a` and one in `upgrade b`.
3. **A terminating gateway-console pod in phase B.** After `helm upgrade`, the old gateway console
   pods can still be Running with a `deletionTimestamp`. An exec into one can fail until the
   deadline. Check: harness row S3 and mutation MB in Task 2 Step 5. Task 6 Step 4 asserts that the
   `upgrade b` log has exactly one `reaches IAM` line, for an `iam-console` pod.
4. **The watchdog stops the wrong process or holds the step open.** If the bounded command is a
   shell function, SIGTERM stops only a subshell, and `kubectl` keeps running. If the watchdog's
   `sleep` keeps the caller's stdout, a capture or the GitHub step waits up to 20 s more. Check:
   Task 1 rows P3 and P4 and the mutations M1 and M2. Task 2 calls the `kubectl` binary, not `k`.
   Task 6 Step 5 compares the step end time with the `== install a: done ==` line.
5. **The exec into the distroless console fails in the cluster.** The path `/nodejs/bin/node`, the
   container `console`, UID 65532, no `-i`/`-t`, and the one single-quoted `PROBE_JS` string all
   matter only in a real pod. Check: Task 2 Step 3 counts the single quotes in `PROBE_JS`. Task 5
   R3 needs a working exec to see HTTP 404, and Task 6 needs it to see HTTP 200.

## Deviations from the spec text

The spec is not changed. The plan implements its intent in five places where the literal text does
not work or leaves a gap. The coordinator confirms these at the plan review.

- **D-a, the console Deployment set.** Spec §4.1 Part B step 2 says `k get deployment -l …`. The
  chart's Deployment metadata has no labels (`charts/paigasus/templates/console-deployment.yaml`
  lines 9-10; `run.sh` `specs()` states the same). So the plan reads
  `.spec.template.metadata.labels` of every Deployment and keeps the `*-console` names of
  `$RELEASE`. It also requires `iam-console` in the set, so an empty result is rc 2, not a pass.
- **D-b, terminating pods.** A pod with a `deletionTimestamp` is not checked and does not count for
  coverage (Review Focus 3).
- **D-c, non-numeric probe output.** Output that is not `000`, empty, or a three-digit status is
  rc 2 ("printed '…', not an HTTP status"), not rc 1. Spec §5 has no row for it.
- **D-d, extra log lines.** The helper prints `iam settle: console Deployments: …`, and each
  `settled: <pod> …` line also prints the seconds for that pod. Task 6 needs both.
- **D-e, the rc 2 message.** It names the last `kubectl exec` rc, so rc 143 shows that the 20 s
  bound stopped the exec.

---

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `ci/kind/run.sh` | modify | `run_bounded` (Task 1); `PROBE_JS`, `settle_iam_service`, the two call sites, the `diagnose` EndpointSlices line (Task 2); the header lines 12 and 18-19 (Task 3) |
| `.github/workflows/chart.yml` | modify | the comments and values of the job timeout and the two step timeouts (Task 3) |
| `ci/kind/README.md` | modify | the mode table, the new section "The IAM settle (SMA-701)", the evidence list, "Where to look first" (Task 3) |

No other tracked file changes. All proof scripts live in `$SCRATCH`.

---

### Task 1: `run_bounded`, the watchdog-bounded command runner

**Files:**
- Modify: `ci/kind/run.sh`, after `read_state()` (lines 101-107 today), before the comment block of `chart_resource_name` (line 109).

- [ ] **Step 1: Write the proof script first.**

Write `$SCRATCH/run-bounded-proof.sh` with this content:

```bash
#!/usr/bin/env bash
# Proof for run_bounded. $1 = a file that holds ONLY the run_bounded function.
set -euo pipefail
FN="$1"
# shellcheck source=/dev/null
. "$FN"
W="$(mktemp -d)"
fails=0
ok() { printf 'ok   %s\n' "$1"; }
bad() { printf 'FAIL %s\n' "$1"; fails=$((fails + 1)); }

# P1: a fast command returns its stdout and status 0, at once.
t0=$SECONDS; rc=0
run_bounded 20 "$W/o" "$W/e" printf '200' || rc=$?
out="$(cat "$W/o")"; dt=$((SECONDS - t0))
if [ "$rc" = 0 ] && [ "$out" = 200 ] && [ "$dt" -le 2 ]; then ok "P1 fast: rc=$rc out=$out ${dt}s"; else bad "P1 fast: rc=$rc out=$out ${dt}s"; fi

# P2: a non-zero status comes back to the caller, and the caller's `|| rc=$?` keeps set -e quiet.
rc=0
run_bounded 20 "$W/o" "$W/e" sh -c 'echo boom >&2; exit 7' || rc=$?
err="$(cat "$W/e")"
if [ "$rc" = 7 ] && [ "$err" = boom ]; then ok "P2 status: rc=$rc err=$err"; else bad "P2 status: rc=$rc err=$err"; fi

# P3: the real 20 s bound kills a stalled stand-in, and the caller continues.
t0=$SECONDS; rc=0
run_bounded 20 "$W/o" "$W/e" sleep 61 || rc=$?
dt=$((SECONDS - t0))
left=0; pgrep -f '^sleep 61$' >/dev/null 2>&1 && left=1
if [ "$rc" = 143 ] && [ "$dt" -ge 19 ] && [ "$dt" -le 23 ] && [ "$left" = 0 ]; then ok "P3 bound: rc=$rc ${dt}s, no sleep 61 left"; else bad "P3 bound: rc=$rc ${dt}s left=$left"; fi

# P4: the watchdog does not hold the caller's stdout. A fast command in $( ) returns at once,
# although the watchdog's own sleep runs on for 30 s.
t0=$SECONDS
got="$(run_bounded 30 "$W/o" "$W/e" true; echo done)"
dt=$((SECONDS - t0))
if [ "$got" = done ] && [ "$dt" -le 2 ]; then ok "P4 no pipe hold: ${dt}s"; else bad "P4 no pipe hold: got=$got ${dt}s"; fi

# P5 (negative control): an UNGUARDED non-zero status ends a set -e script. This proves that the
# status is not swallowed, so the `|| true` at the call site is load-bearing.
rc=0
"$BASH" -c 'set -euo pipefail; . "$1"; run_bounded 5 /dev/null /dev/null false; echo NOT_REACHED' _ "$FN" >"$W/p5" 2>&1 || rc=$?
if [ "$rc" = 1 ] && [ ! -s "$W/p5" ]; then ok "P5 negative control: rc=$rc, nothing printed"; else bad "P5 negative control: rc=$rc out=$(cat "$W/p5")"; fi

# P6: a guarded call in a set -e script continues after a killed command.
rc=0
"$BASH" -c 'set -euo pipefail; . "$1"; run_bounded 2 /dev/null /dev/null sleep 62 || true; echo CONTINUED' _ "$FN" >"$W/p6" 2>&1 || rc=$?
if [ "$rc" = 0 ] && [ "$(cat "$W/p6")" = CONTINUED ]; then ok "P6 continue: rc=$rc"; else bad "P6 continue: rc=$rc out=$(cat "$W/p6")"; fi

rm -rf "$W"
printf '%s: %s failure(s)\n' "$BASH_VERSION" "$fails"
[ "$fails" = 0 ]
```

The script does not source `run.sh`. It sources only the function, which Step 3 extracts. So the
dispatch of `run.sh` never runs.

- [ ] **Step 2: Run the proof before the function exists. Expect an infrastructure failure.**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-701-kind-iam-settle
sed -n '/^run_bounded() {/,/^}/p' ci/kind/run.sh >"$SCRATCH/run_bounded.sh"
/bin/bash "$SCRATCH/run-bounded-proof.sh" "$SCRATCH/run_bounded.sh"; echo "rc=$?"
```

Expected: `P1` fails with `run_bounded: command not found` in the output, and the last line is
`rc=1` or higher. The file `$SCRATCH/run_bounded.sh` is empty.

- [ ] **Step 3: Add the function.**

In `ci/kind/run.sh`, find this exact text (the end of `read_state`):

```bash
  [ -n "$v" ] || return 1
  printf '%s' "$v"
}

# The name the chart gives a resource: charts/paigasus/templates/_helpers.tpl "paigasus.name".
```

Replace it with:

```bash
  [ -n "$v" ] || return 1
  printf '%s' "$v"
}

# Runs an EXTERNAL command ($4 onward) with stdin from /dev/null, stdout into the file $2 and
# stderr into the file $3. A watchdog sends SIGTERM to it after $1 seconds, so a stalled command
# cannot hold the caller (macOS has no timeout(1)). Returns the command's exit status: 143 when
# the watchdog stopped it. Pass a binary, not a shell function: a function runs in a subshell, and
# SIGTERM would stop only that subshell. The watchdog's stdio is /dev/null, so its sleep never
# holds the caller's stdout open (SMA-701).
run_bounded() {  # $1 = seconds, $2 = stdout file, $3 = stderr file, then the command
  local secs="$1" out="$2" err="$3" pid wd rc=0
  shift 3
  "$@" </dev/null >"$out" 2>"$err" &
  pid=$!
  ( sleep "$secs"; kill "$pid" 2>/dev/null ) </dev/null >/dev/null 2>&1 &
  wd=$!
  wait "$pid" 2>/dev/null || rc=$?
  kill "$wd" 2>/dev/null || true
  wait "$wd" 2>/dev/null || true
  return "$rc"
}

# The name the chart gives a resource: charts/paigasus/templates/_helpers.tpl "paigasus.name".
```

- [ ] **Step 4: Run the proof under both bash versions. Expect a pass.**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-701-kind-iam-settle
sed -n '/^run_bounded() {/,/^}/p' ci/kind/run.sh >"$SCRATCH/run_bounded.sh"
/bin/bash "$SCRATCH/run-bounded-proof.sh" "$SCRATCH/run_bounded.sh"; echo "rc=$?"
/opt/homebrew/bin/bash "$SCRATCH/run-bounded-proof.sh" "$SCRATCH/run_bounded.sh"; echo "rc=$?"
```

Expected, for each bash (measured on 2026-10-02 with 3.2.57 and 5.3.20):

```
ok   P1 fast: rc=0 out=200 0s
ok   P2 status: rc=7 err=boom
ok   P3 bound: rc=143 20s, no sleep 61 left
ok   P4 no pipe hold: 0s
ok   P5 negative control: rc=1, nothing printed
ok   P6 continue: rc=0
3.2.57(1)-release: 0 failure(s)
rc=0
```

Under bash 5 the summary line starts with `5.3.20(1)-release`. P3 may show 19-23 s. Each run takes
about 25 s.

- [ ] **Step 5: Prove that P3 and P4 can fail (delete the feature).**

```bash
sed '/( sleep "\$secs"; kill/d' "$SCRATCH/run_bounded.sh" >"$SCRATCH/run_bounded.m1.sh"
sed 's|) </dev/null >/dev/null 2>&1 &|) \&|' "$SCRATCH/run_bounded.sh" >"$SCRATCH/run_bounded.m2.sh"
/bin/bash "$SCRATCH/run-bounded-proof.sh" "$SCRATCH/run_bounded.m1.sh"; echo "rc=$?"
/opt/homebrew/bin/bash "$SCRATCH/run-bounded-proof.sh" "$SCRATCH/run_bounded.m2.sh"; echo "rc=$?"
```

Expected (measured on 2026-10-02):
- M1 (no watchdog): `FAIL P3 bound: rc=0 61s left=0`, all other rows `ok`, `1 failure(s)`, `rc=1`. It takes about 65 s.
- M2 (the watchdog keeps the caller's stdout), under bash 5: `FAIL P4 no pipe hold: got=done 30s`, `1 failure(s)`, `rc=1`.
- Under `/bin/bash` 3.2.57, M2 does NOT fail P4 (measured). So P4 is a real test only under bash 5. CI uses bash 5. Do not report M2 under 3.2 as a defect.

Do not edit `run.sh` for a mutation. The mutations change only the scratch copies.

- [ ] **Step 6: Static checks.**

```bash
/bin/bash -n ci/kind/run.sh; echo "bash3 rc=$?"
/opt/homebrew/bin/bash -n ci/kind/run.sh; echo "bash5 rc=$?"
uv run --locked --project py shellcheck ci/kind/run.sh; echo "shellcheck rc=$?"
```

Expected: `bash3 rc=0`, `bash5 rc=0`, `shellcheck rc=0` with no finding. shellcheck does not report
an unused function, so the function alone is clean (measured).

- [ ] **Step 7: Commit.**

```bash
git add ci/kind/run.sh
git commit -F "$SCRATCH/msg-task1.txt"
```

with `$SCRATCH/msg-task1.txt`:

```
feat(ci): add a watchdog-bounded command runner to the kind script (SMA-701)

run_bounded runs a binary with stdout and stderr in files and stops it
with SIGTERM after a bound. macOS has no timeout(1). The watchdog uses
/dev/null for its stdio, so it never holds the caller's output open.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

Check: `git show --stat HEAD` lists only `ci/kind/run.sh`.

---

### Task 2: `settle_iam_service`, its call sites and the `diagnose` EndpointSlices

**Files:**
- Modify: `ci/kind/run.sh`: after `settle_gateway_404` (ends at line 443 today, before Task 1's
  insert); `install_a` (lines 397-398 today); `upgrade_b` (lines 479-480 today); `diagnose`
  (after line 667 today).

- [ ] **Step 1: Write the PROBE_JS proof script.**

Write `$SCRATCH/probe-js-proof.sh`:

```bash
#!/usr/bin/env bash
# $1 = a file that holds ONLY the PROBE_JS assignment.
set -uo pipefail
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
# shellcheck source=/dev/null
. "$1"
W="$(mktemp -d)"
fails=0
# A server: /readyz on port A answers 200, every path on port B answers 404, port C accepts and
# never answers. It prints the three ports.
node -e '
var http = require("http");
var a = http.createServer(function (q, r) { r.writeHead(q.url === "/readyz" ? 200 : 404); r.end("{\"status\":\"ok\"}"); });
var b = http.createServer(function (q, r) { r.writeHead(404); r.end("nope"); });
var c = http.createServer(function () {});
a.listen(0, "127.0.0.1", function () { b.listen(0, "127.0.0.1", function () { c.listen(0, "127.0.0.1", function () {
  console.log(a.address().port + " " + b.address().port + " " + c.address().port);
}); }); });' >"$W/ports" &
srv=$!
i=0; while [ ! -s "$W/ports" ] && [ "$i" -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
read -r PA PB PC <"$W/ports"
# A closed port: bind, read the port, close.
PD="$(node -e 'var s=require("net").createServer();s.listen(0,"127.0.0.1",function(){var p=s.address().port;s.close(function(){console.log(p)})})')"
check() {  # $1 = label, $2 = expected output, $3 = PAIGASUS_SERVICES value or the word UNSET
  local got rc=0 t0=$SECONDS
  if [ "$3" = UNSET ]; then
    got="$(env -u PAIGASUS_SERVICES node -e "$PROBE_JS")" || rc=$?
  else
    got="$(PAIGASUS_SERVICES="$3" node -e "$PROBE_JS")" || rc=$?
  fi
  if [ "$got" = "$2" ] && [ "$rc" = 0 ]; then
    printf 'ok   %s: %s rc=%s %ss\n' "$1" "$got" "$rc" "$((SECONDS - t0))"
  else
    printf 'FAIL %s: got [%s] rc=%s, want [%s] rc=0\n' "$1" "$got" "$rc" "$2"; fails=$((fails + 1))
  fi
}
check "200 server" 200 "{\"iam\":\"http://127.0.0.1:$PA\"}"
check "404 server" 404 "{\"iam\":\"http://127.0.0.1:$PB\"}"
check "closed port" 000 "{\"iam\":\"http://127.0.0.1:$PD\"}"
check "no answer (5 s abort)" 000 "{\"iam\":\"http://127.0.0.1:$PC\"}"
check "variable unset" 000 UNSET
check "bad JSON" 000 "not json"
check "no iam key" 000 "{\"gateway\":\"http://127.0.0.1:$PA\"}"
check "two zones" 200 "{\"gateway\":\"http://127.0.0.1:$PB\",\"iam\":\"http://127.0.0.1:$PA\"}"
kill "$srv" 2>/dev/null; wait "$srv" 2>/dev/null
rm -rf "$W"
printf 'PROBE_JS: %s failure(s)\n' "$fails"
[ "$fails" = 0 ]
```

- [ ] **Step 2: Write the offline helper proof (a fake `kubectl`, no cluster).**

Write `$SCRATCH/settle-proof.sh`:

```bash
#!/usr/bin/env bash
# Offline proof of settle_iam_service with a fake kubectl. No cluster.
# $1 = the run.sh to test, $2 = the repo root (for charts/paigasus/Chart.yaml), $3 = fast | all
set -uo pipefail
RUN_SH="$1"; ROOT="$2"; WHICH="${3:-fast}"
W="$(mktemp -d)"
mkdir -p "$W/bin" "$W/state"
# Everything in run.sh before the dispatch: the functions and the globals, not the `case "$cmd"`.
sed '/^cmd="\${1:-}"$/,$d' "$RUN_SH" >"$W/lib.sh"
grep -q '^settle_iam_service() {' "$W/lib.sh" || { echo "INFRA: no settle_iam_service in $RUN_SH"; exit 2; }

# The fake kubectl. FAKE_* variables choose its answers; it logs each exec'd pod.
printf '%s\n' '#!/usr/bin/env bash' \
'args="$*"' \
'next() {  # $1 = counter name, $2 = space-separated sequence; prints the next item, the last repeats' \
'  local n=0 i=0 v last=""' \
'  [ -f "$FAKE_DIR/$1.n" ] && n="$(cat "$FAKE_DIR/$1.n")"' \
'  echo $((n + 1)) >"$FAKE_DIR/$1.n"' \
'  for v in $2; do last="$v"; if [ "$i" = "$n" ]; then printf "%s" "$v"; return; fi; i=$((i + 1)); done' \
'  printf "%s" "$last"' \
'}' \
'case "$args" in' \
'  *" get deployment "*" -o name")' \
'    [ "${FAKE_IAM_MISSING:-0}" = 1 ] && { echo "Error from server (NotFound)" >&2; exit 1; }' \
'    echo "deployment.apps/${args##* get deployment }" | sed "s/ -o name\$//" ;;' \
'  *" get deployment "*"readyReplicas"*) next ready "${FAKE_READY_SEQ:-1}" ;;' \
'  *" rollout status "*) exit "${FAKE_ROLLOUT_RC:-0}" ;;' \
'  *" get deployments "*) printf "%s" "${FAKE_DEPS-}" ;;' \
'  *" get pods "*) printf "%s" "${FAKE_PODS-}" ;;' \
'  *" exec -c console "*)' \
'    pod="${args#* exec -c console }"; pod="${pod%% *}"' \
'    echo "$pod" >>"$FAKE_DIR/exec.log"' \
'    case "$args" in *"-- /nodejs/bin/node -e Promise.resolve()"*) ;; *) echo "fake: bad exec argv" >&2; exit 99 ;; esac' \
'    m="$(next exec "${FAKE_EXEC_SEQ:-200}")"' \
'    case "$m" in' \
'      hang) exec sleep 61 ;;' \
'      fail) echo "error: unable to upgrade connection: container not found" >&2; exit 1 ;;' \
'      junk) echo hello ;;' \
'      *) echo "$m" ;;' \
'    esac ;;' \
'  *) echo "fake kubectl: unexpected call: $args" >&2; exit 98 ;;' \
'esac' >"$W/bin/kubectl"
chmod +x "$W/bin/kubectl"

# The driver: source the functions, point REPO_ROOT at the real repo, run the helper.
printf '%s\n' \
'. "$1"' \
'REPO_ROOT="$2"' \
'settle_iam_service' \
'echo DRIVER_DONE' >"$W/driver.sh"

DEPS_A='gateway-console/paigasus iam-console/paigasus iam-backend/paigasus gateway-stub/ '
PODS_A='p-gw/gateway-console/ p-iam/iam-console/ '
fails=0
# $1 label, $2 want rc, $3 want text in the output, then VAR=value settings for the fake.
case_() {
  local label="$1" want_rc="$2" want="$3" rc=0 t0=$SECONDS out
  shift 3
  rm -f "$W"/fake/* 2>/dev/null; mkdir -p "$W/fake"
  out="$(env PATH="$W/bin:$PATH" PAIGASUS_KIND_STATE="$W/state" FAKE_DIR="$W/fake" "$@" \
    "$BASH" "$W/driver.sh" "$W/lib.sh" "$ROOT" 2>&1)" || rc=$?
  case "$out" in *"$want"*) found=1 ;; *) found=0 ;; esac
  if [ "$rc" = "$want_rc" ] && [ "$found" = 1 ]; then
    printf 'ok   %-34s rc=%s %3ss\n' "$label" "$rc" "$((SECONDS - t0))"
  else
    printf 'FAIL %-34s rc=%s (want %s) %ss\n--- output:\n%s\n---\n' "$label" "$rc" "$want_rc" "$((SECONDS - t0))" "$out"
    fails=$((fails + 1))
  fi
  LAST_OUT="$out"
}

case_ "S1 phase A, both pods 200" 0 "DRIVER_DONE" FAKE_DEPS="$DEPS_A" FAKE_PODS="$PODS_A" FAKE_READY_SEQ=0
case "$LAST_OUT" in *"IAM readyReplicas=0"*"console Deployments: gateway-console iam-console"*"settled: p-gw reaches"*"settled: p-iam reaches"*) ;;
  *) echo "FAIL S1 lines: $LAST_OUT"; fails=$((fails + 1)) ;; esac
case_ "S2 gateway pod missing" 2 "no Running gateway-console pod" FAKE_DEPS="$DEPS_A" FAKE_PODS='p-iam/iam-console/ '
case_ "S3 phase B, dying gateway pod" 0 "DRIVER_DONE" FAKE_DEPS='iam-console/paigasus iam-backend/paigasus gateway-stub/ ' \
  FAKE_PODS='p-gw-dying/gateway-console/2026-10-02T17:35:32Z p-iam/iam-console/ '
if [ "$(cat "$W/fake/exec.log")" != p-iam ]; then echo "FAIL S3: exec'd pods: $(cat "$W/fake/exec.log")"; fails=$((fails + 1)); fi
case_ "S4 deployments jsonpath empty" 2 "no iam-console Deployment" FAKE_DEPS='' FAKE_PODS="$PODS_A"
case_ "S5 only a dying iam pod" 2 "no Running iam-console pod" FAKE_DEPS='iam-console/paigasus ' FAKE_PODS='p-iam/iam-console/2026-10-02T17:35:32Z '
case_ "S6 IAM Deployment missing" 2 "cannot find the IAM backend Deployment" FAKE_IAM_MISSING=1
case_ "S7 rollout fails" 1 "did not become Ready in 240 s" FAKE_ROLLOUT_RC=1
case_ "S8 000 twice, then 200" 0 "DRIVER_DONE" FAKE_DEPS="$DEPS_A" FAKE_PODS="$PODS_A" FAKE_EXEC_SEQ='000 fail 200'
case_ "S9 other release ignored" 0 "console Deployments: iam-console" FAKE_DEPS='iam-console/paigasus gateway-console/other ' FAKE_PODS='p-iam/iam-console/ '
if [ "$WHICH" = all ]; then
  case_ "D1 404 at the deadline" 1 "answers HTTP 404" FAKE_DEPS="$DEPS_A" FAKE_PODS="$PODS_A" FAKE_EXEC_SEQ=404
  case_ "D2 000, IAM still Ready" 2 "p-gw cannot reach IAM" FAKE_DEPS="$DEPS_A" FAKE_PODS="$PODS_A" FAKE_EXEC_SEQ=000 FAKE_READY_SEQ='0 1'
  case_ "D3 000, IAM NotReady" 1 "IAM became NotReady after the rollout" FAKE_DEPS="$DEPS_A" FAKE_PODS="$PODS_A" FAKE_EXEC_SEQ=000 FAKE_READY_SEQ='1 0'
  case_ "D4 exec hangs (20 s bound)" 2 "last kubectl exec rc 143" FAKE_DEPS="$DEPS_A" FAKE_PODS="$PODS_A" FAKE_EXEC_SEQ=hang
  case_ "D5 exec fails, stderr shown" 2 "container not found" FAKE_DEPS="$DEPS_A" FAKE_PODS="$PODS_A" FAKE_EXEC_SEQ=fail
  case_ "D6 junk output" 2 "printed 'hello', not an HTTP status" FAKE_DEPS="$DEPS_A" FAKE_PODS="$PODS_A" FAKE_EXEC_SEQ=junk
fi
rm -rf "$W"
printf '%s (%s): %s failure(s)\n' "$BASH_VERSION" "$WHICH" "$fails"
[ "$fails" = 0 ]
```

The harness runs the real `on_exit` trap, `die_infra` and `die_assert`, so it checks the rc
classes as CI sees them. The fake `kubectl` is an executable file, because `run_bounded` needs a
binary.

Run it now, before the helper exists:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-701-kind-iam-settle
/bin/bash "$SCRATCH/settle-proof.sh" ci/kind/run.sh "$PWD" fast; echo "rc=$?"
```

Expected: `INFRA: no settle_iam_service in ci/kind/run.sh` and `rc=2`.

- [ ] **Step 3: Add `PROBE_JS` and `settle_iam_service`.**

In `ci/kind/run.sh`, find this exact text (the end of `settle_gateway_404` and the start of
`upgrade_b`):

```bash
  echo "  settled: /gateway/overview answers Traefik's 404"
}

upgrade_b() {
```

Replace it with:

```bash
  echo "  settled: /gateway/overview answers Traefik's 404"
}

# SMA-701: the IAM settle. Helm 3.22.0's --wait counts a Deployment as ready when readyReplicas >=
# replicas - maxUnavailable. The IAM backend has replicas 1 and maxUnavailable 1 on purpose, so
# `helm install --wait` can return while IAM has no Ready pod and its Service has no endpoint.
# `kubectl wait --for=condition=Available` has the same floor; `rollout status` does not
# (ci/kind/README.md, "The IAM settle"). Part A waits for the rollout. Part B asks IAM /readyz
# through its Service from each console pod, because the discovery probe runs in that pod.
#
# PROBE_JS runs in a console pod with `node -e`: CommonJS and a promise chain, no top-level await.
# It reads the IAM URL from PAIGASUS_SERVICES, the variable the console reads, so it holds no copy
# of the Service name or the port. It prints the HTTP status of GET <iam>/readyz, or 000 on any
# error, and always exits 0, so a failed fetch never reads as a failed exec. It holds no single
# quote: it is one single-quoted bash string.
PROBE_JS='Promise.resolve().then(function () {
  var iam = JSON.parse(process.env.PAIGASUS_SERVICES).iam;
  if (typeof iam !== "string" || iam === "") { throw new Error("PAIGASUS_SERVICES has no iam URL"); }
  return fetch(iam + "/readyz", { signal: AbortSignal.timeout(5000) });
}).then(function (res) {
  var done = res.body ? res.body.cancel().catch(function () {}) : null;
  return Promise.resolve(done).then(function () { return String(res.status); });
}).then(function (code) {
  process.stdout.write(code + "\n", function () { process.exit(0); });
}, function () {
  process.stdout.write("000\n", function () { process.exit(0); });
});'

settle_iam_service() {
  local start iam_deploy ready deps d dname dinst expected pods e pod rest pname del found deadline pstart xrc code now
  start="$(date +%s)"
  # Entry: the names, and the line that shows whether Helm returned before IAM was Ready.
  iam_deploy="$(chart_resource_name iam-backend)" \
    || die_infra "cannot find the IAM backend Deployment: cannot derive its name from charts/paigasus/Chart.yaml"
  k -n "$NS" get deployment "$iam_deploy" -o name >/dev/null \
    || die_infra "cannot find the IAM backend Deployment $iam_deploy in $NS"
  ready="$(k -n "$NS" get deployment "$iam_deploy" -o jsonpath='{.status.readyReplicas}')" \
    || die_infra "cannot read the readyReplicas of $iam_deploy"
  echo "  iam settle: start $(date -u +%H:%M:%S) UTC, IAM readyReplicas=${ready:-0}"

  # Part A: the IAM pod is Ready. rollout status has no maxUnavailable floor. 240 s: the startup
  # probe (60 s), the migration lock wait (120 s by default), the migration, one readiness period.
  k -n "$NS" rollout status "deployment/$iam_deploy" --timeout=240s \
    || die_assert "the IAM backend Deployment did not become Ready in 240 s"
  echo "  settled: IAM Ready after $(( $(date +%s) - start )) s"

  # Part B, step 1: the console Deployments of this release that exist now. By the POD TEMPLATE
  # labels: the chart's Deployment metadata has no labels (console-deployment.yaml), so a
  # `get deployment -l` finds nothing.
  deps="$(k -n "$NS" get deployments \
    -o jsonpath='{range .items[*]}{.spec.template.metadata.labels.app\.kubernetes\.io/name}{"/"}{.spec.template.metadata.labels.app\.kubernetes\.io/instance}{" "}{end}')" \
    || die_infra "cannot list the Deployments in $NS"
  expected=""
  for d in $deps; do
    dname="${d%%/*}"
    dinst="${d#*/}"
    case "$dname" in
      *-console) if [ "$dinst" = "$RELEASE" ]; then expected="$expected $dname"; fi ;;
    esac
  done
  # A jsonpath that matches nothing must not skip Part B in silence: iam-console exists in phase A
  # and in phase B.
  case "$expected " in
    *" iam-console "*) ;;
    *) die_infra "no iam-console Deployment of release $RELEASE in $NS (found:${expected:- none}), so Part B has nothing to check" ;;
  esac
  echo "  iam settle: console Deployments:$expected"

  # Step 2: the Running console pods. A pod with a deletionTimestamp is shutting down (phase B:
  # the gateway console pods after the upgrade removed their Deployment), so it is not checked and
  # it does not count for coverage. Each console Deployment needs one pod to check.
  pods="$(k -n "$NS" get pods \
    -l "app.kubernetes.io/name in (gateway-console,iam-console),app.kubernetes.io/instance=$RELEASE" \
    --field-selector=status.phase=Running \
    -o jsonpath='{range .items[*]}{.metadata.name}{"/"}{.metadata.labels.app\.kubernetes\.io/name}{"/"}{.metadata.deletionTimestamp}{" "}{end}')" \
    || die_infra "cannot list the console pods in $NS"
  for dname in $expected; do
    found=0
    for e in $pods; do
      rest="${e#*/}"
      pname="${rest%%/*}"
      del="${rest#*/}"
      if [ "$pname" = "$dname" ] && [ -z "$del" ]; then found=1; fi
    done
    [ "$found" = 1 ] || die_infra "no Running $dname pod to check the IAM data path from"
  done

  # Step 3: poll each pod until IAM answers 200, with a 60 s deadline per pod.
  for e in $pods; do
    pod="${e%%/*}"
    rest="${e#*/}"
    del="${rest#*/}"
    [ -z "$del" ] || continue
    pstart="$(date +%s)"
    deadline=$(( pstart + 60 ))
    while :; do
      # A stale answer from an earlier iteration must not read as this one's.
      rm -f "$STATE/settle-iam.out" "$STATE/settle-iam.err"
      xrc=0
      # The kubectl binary, not k(): run_bounded must signal the command itself, not a subshell.
      run_bounded 20 "$STATE/settle-iam.out" "$STATE/settle-iam.err" \
        kubectl --context "$CONTEXT" -n "$NS" exec -c console "$pod" -- /nodejs/bin/node -e "$PROBE_JS" || xrc=$?
      code="$(cat "$STATE/settle-iam.out" 2>/dev/null || true)"
      [ "$code" != 200 ] || break
      if [ "$(date +%s)" -ge "$deadline" ]; then
        case "$code" in
          ''|000)
            ready="$(k -n "$NS" get deployment "$iam_deploy" -o jsonpath='{.status.readyReplicas}')" \
              || die_infra "cannot read the readyReplicas of $iam_deploy"
            if [ "${ready:-0}" = 0 ]; then
              die_assert "IAM became NotReady after the rollout: $iam_deploy has readyReplicas=0, and $pod had no answer from IAM /readyz for 60 s"
            fi
            die_infra "$pod cannot reach IAM /readyz through its Service in 60 s (last kubectl exec rc $xrc; 143 is the 20 s exec bound): $(cat "$STATE/settle-iam.err" 2>/dev/null || true)" ;;
          [1-5][0-9][0-9])
            die_assert "$pod reaches IAM, but it answers HTTP $code on /readyz 60 s after the IAM rollout; want 200" ;;
          *)
            die_infra "the IAM probe in $pod printed '$code', not an HTTP status (last kubectl exec rc $xrc): $(cat "$STATE/settle-iam.err" 2>/dev/null || true)" ;;
        esac
      fi
      sleep 2
    done
    now="$(date +%s)"
    echo "  settled: $pod reaches IAM /readyz (HTTP 200) after $(( now - start )) s ($(( now - pstart )) s for this pod)"
  done
}

upgrade_b() {
```

Then check the quoting of `PROBE_JS`:

```bash
sed -n "/^PROBE_JS='/,/^});'\$/p" ci/kind/run.sh >"$SCRATCH/probe_js.sh"
grep -o "'" "$SCRATCH/probe_js.sh" | wc -l
```

Expected: `2` (the opening quote and the closing quote only). The file has 12 lines.

- [ ] **Step 4: Add the two call sites and the `diagnose` line.**

In `install_a`, find:

```bash
  echo "  phase A: $gw exists"
  assert_notes_marker "install a"
```

Replace it with:

```bash
  echo "  phase A: $gw exists"
  settle_iam_service
  assert_notes_marker "install a"
```

In `upgrade_b`, find:

```bash
  settle_gateway_404
  assert_notes_marker "upgrade b"
```

Replace it with:

```bash
  settle_gateway_404
  settle_iam_service
  assert_notes_marker "upgrade b"
```

In `diagnose`, find:

```bash
  k -n "$NS" get endpoints "$STUB" -o wide >>"$d/gateway-stub.txt" 2>&1 || true
```

Replace it with:

```bash
  k -n "$NS" get endpoints "$STUB" -o wide >>"$d/gateway-stub.txt" 2>&1 || true
  # SMA-701: `get all` lists no EndpointSlice. An IAM settle rc 2 ("cannot reach IAM") needs them.
  k -n "$NS" get endpointslices -o wide >"$d/endpointslices.txt" 2>&1 || true
```

- [ ] **Step 5: Run the three proofs. Expect a pass.**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-701-kind-iam-settle
/bin/bash "$SCRATCH/probe-js-proof.sh" "$SCRATCH/probe_js.sh"; echo "rc=$?"
/opt/homebrew/bin/bash "$SCRATCH/probe-js-proof.sh" "$SCRATCH/probe_js.sh"; echo "rc=$?"
/opt/homebrew/bin/bash "$SCRATCH/settle-proof.sh" ci/kind/run.sh "$PWD" fast; echo "rc=$?"
/bin/bash "$SCRATCH/settle-proof.sh" ci/kind/run.sh "$PWD" all; echo "rc=$?"
```

Expected (measured on 2026-10-02 against the same code in a scratch copy):
- `probe-js-proof.sh`, each bash: 8 rows `ok` with outputs `200`, `404`, `000`, `000` (about
  5 s), `000`, `000`, `000`, `200`; `PROBE_JS: 0 failure(s)`; `rc=0`. The local node is 24.16.0,
  the same major as the console image (`gcr.io/distroless/nodejs24-debian12`).
- `settle-proof.sh … fast` under bash 5: rows S1-S9 `ok`, `0 failure(s)`, `rc=0`, about 10 s.
- `settle-proof.sh … all` under `/bin/bash`: rows S1-S9 and D1-D6 `ok`, `0 failure(s)`, `rc=0`.
  D1, D2, D3, D5 and D6 take about 61 s each; D4 took 64 s (it can take up to about 88 s). The
  whole run takes about 7 minutes. Run it with a 600000 ms tool timeout, or in the background and read the output file.

Then delete two features, and expect a failure:

```bash
sed '/\[ "\$found" = 1 \] || die_infra/d' ci/kind/run.sh >"$SCRATCH/run.mA.sh"
sed '/^    \[ -z "\$del" \] || continue$/d' ci/kind/run.sh >"$SCRATCH/run.mB.sh"
/bin/bash "$SCRATCH/settle-proof.sh" "$SCRATCH/run.mA.sh" "$PWD" fast; echo "rc=$?"
/bin/bash "$SCRATCH/settle-proof.sh" "$SCRATCH/run.mB.sh" "$PWD" fast; echo "rc=$?"
```

Expected (measured): MA gives `FAIL S2 gateway pod missing rc=0 (want 2)` and
`FAIL S5 only a dying iam pod rc=0 (want 2)`, `2 failure(s)`. MB gives
`FAIL S3: exec'd pods: p-gw-dying` (and `p-iam` on the next line), `1 failure(s)`. Each mutated
file is one line shorter than `ci/kind/run.sh`; check with `wc -l`, so a sed that matched nothing
cannot pass as a mutation.

- [ ] **Step 6: Check the two jsonpath templates with a real kubectl (offline).**

The fake `kubectl` ignores the jsonpath (Review Focus 1). Write `$SCRATCH/list.yaml`:

```yaml
apiVersion: v1
kind: List
items:
- apiVersion: v1
  kind: Pod
  metadata:
    name: paigasus-paigasus-iam-console-abc
    labels: {app.kubernetes.io/name: iam-console, app.kubernetes.io/instance: paigasus}
- apiVersion: v1
  kind: Pod
  metadata:
    name: paigasus-paigasus-gateway-console-def
    deletionTimestamp: "2026-10-02T17:35:32Z"
    labels: {app.kubernetes.io/name: gateway-console, app.kubernetes.io/instance: paigasus}
- apiVersion: apps/v1
  kind: Deployment
  metadata: {name: paigasus-paigasus-iam-console}
  spec:
    selector: {matchLabels: {app.kubernetes.io/name: iam-console}}
    template:
      metadata:
        labels: {app.kubernetes.io/name: iam-console, app.kubernetes.io/instance: paigasus}
- apiVersion: apps/v1
  kind: Deployment
  metadata: {name: gateway-stub}
  spec:
    selector: {matchLabels: {app.kubernetes.io/name: gateway-stub}}
    template:
      metadata:
        labels: {app.kubernetes.io/name: gateway-stub}
```

Run (no cluster is needed; `annotate --local` applies the template to each item):

```bash
kubectl annotate --local -f "$SCRATCH/list.yaml" x=y -o jsonpath='{.metadata.name}{"/"}{.metadata.labels.app\.kubernetes\.io/name}{"/"}{.metadata.deletionTimestamp}{" "}'; echo "| rc=$?"
kubectl annotate --local -f "$SCRATCH/list.yaml" x=y -o jsonpath='{.spec.template.metadata.labels.app\.kubernetes\.io/name}{"/"}{.spec.template.metadata.labels.app\.kubernetes\.io/instance}{" "}'; echo "| rc=$?"
```

Expected (measured with the host kubectl 1.36.1; CI uses 1.31.14):

```
paigasus-paigasus-iam-console-abc/iam-console/ paigasus-paigasus-gateway-console-def/gateway-console/2026-10-02T17:35:32Z paigasus-paigasus-iam-console// gateway-stub// | rc=0
/ / iam-console/paigasus gateway-stub/ | rc=0
```

The inner templates are the same text as in `run.sh`, without the `{range .items[*]}…{end}`
wrapper, which `active_hashes` already uses in CI. A pod without labels prints `//`, and the
helper ignores it. Task 6 checks the real selectors in CI.

- [ ] **Step 7: Static checks.**

```bash
/bin/bash -n ci/kind/run.sh; echo "bash3 rc=$?"
/opt/homebrew/bin/bash -n ci/kind/run.sh; echo "bash5 rc=$?"
uv run --locked --project py shellcheck ci/kind/run.sh; echo "shellcheck rc=$?"
```

Expected: three times `rc=0`, and shellcheck prints no finding (measured on the scratch copy).

- [ ] **Step 8: Commit.**

```bash
git add ci/kind/run.sh
git commit -F "$SCRATCH/msg-task2.txt"
```

with `$SCRATCH/msg-task2.txt`:

```
feat(ci): wait for the IAM Service after the kind install and upgrade (SMA-701)

Helm 3.22.0 counts the IAM Deployment as ready with zero Ready pods,
because replicas 1 minus maxUnavailable 1 is 0. settle_iam_service
waits for the IAM rollout, then asks IAM /readyz through its Service
from each console pod until it answers HTTP 200. install a and upgrade b
call it before the NOTES check. diagnose also collects EndpointSlices.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

Check: `git show --stat HEAD` lists only `ci/kind/run.sh`.

---

### Task 3: the step timeouts, the `run.sh` header and the README

**Files:**
- Modify: `.github/workflows/chart.yml` lines 59-68 (job), 146-149 (`install a`), 174-179 (`upgrade b`).
- Modify: `ci/kind/run.sh` header lines 12 and 18-19.
- Modify: `ci/kind/README.md`: the mode table (rows `install a`, `upgrade b`), a new section after
  "The NOTES check (SMA-691)", the evidence list, "Where to look first".

- [ ] **Step 1: The job timeout.** In `.github/workflows/chart.yml`, replace lines 59-68:

```yaml
    # 169 (SMA-514). Each step timeout below is sized to the SUM of ci/kind/run.sh's own
    # --timeout/deadline values for that mode (review Minor 1), because each is a generous allowance
    # for Docker Hub throttling and can legitimately run close to its cap on the same slow pull. The
    # job budget must hold them all, or a slow but passing run is killed by the JOB timeout. The
    # step timeouts sum to 158 minutes: 57 (up) + 25 (images) + 12 (install a) + 10 (specs a) +
    # 9 (stub up) + 10 (specs journeys) + 17 (upgrade b) + 8 (specs b) + 5 (evidence) + 5 (down).
    # Before SMA-514 the sum was 139 and the budget 150; the budget grows by the two new step
    # timeouts (9 + 10), which keeps the same 11-minute room for the untimed steps (reclaim,
    # checkout, tool and dependency installs, Chromium).
    timeout-minutes: 169
```

with:

```yaml
    # 182 (SMA-701; 169 in SMA-514). Each step timeout below is sized to the SUM of
    # ci/kind/run.sh's own --timeout/deadline values for that mode (review Minor 1), because each is
    # a generous allowance for Docker Hub throttling and can legitimately run close to its cap on the
    # same slow pull. The job budget must hold them all, or a slow but passing run is killed by the
    # JOB timeout. The step timeouts sum to 171 minutes: 57 (up) + 25 (images) + 19 (install a) +
    # 10 (specs a) + 9 (stub up) + 10 (specs journeys) + 23 (upgrade b) + 8 (specs b) +
    # 5 (evidence) + 5 (down). Before SMA-514 the sum was 139 and the budget 150; the budget grew by
    # the two new step timeouts (9 + 10). SMA-701 adds the IAM settle: +7 (install a) and
    # +6 (upgrade b), to the sum and to the budget. Both keep the same 11-minute room for the
    # untimed steps (reclaim, checkout, tool and dependency installs, Chromium).
    timeout-minutes: 182
```

- [ ] **Step 2: The `install a` step.** Replace lines 146-149:

```yaml
      - name: helm install (phase A, both zones)
        id: install-a
        timeout-minutes: 12
        run: bash ci/kind/run.sh install a
```

with:

```yaml
      # 19, not 12 (SMA-701). run.sh install_a's own waits, summed: helm install --timeout 10m
      # (10m) + the IAM rollout status --timeout=240s (4m) + the IAM data-path poll, 1.5 min per
      # console pod (the 60-s deadline, one last 20-s exec bound and one 2-s sleep), 2 pods in
      # phase A (3m) = 10+4+3 = 17 minutes. +2 min margin.
      - name: helm install (phase A, both zones)
        id: install-a
        timeout-minutes: 19
        run: bash ci/kind/run.sh install a
```

Do not change the step name. Task 5 reads it.

- [ ] **Step 3: The `upgrade b` step.** Replace lines 174-179:

```yaml
      # 17, not 12 (review Minor 1, the reviewer's own worked example). run.sh upgrade_b's own
      # waits, summed: helm upgrade --timeout 10m (10m) + the old-ReplicaSet settle deadline (3m)
      # + settle_gateway_404's deadline (2m) = 10+3+2 = 15 minutes. +2 min margin.
      - name: helm upgrade (phase B, gateway zone off) and settle
        timeout-minutes: 17
        run: bash ci/kind/run.sh upgrade b
```

with:

```yaml
      # 23 (SMA-701; 17 in review Minor 1, the reviewer's own worked example). run.sh upgrade_b's
      # own waits, summed: helm upgrade --timeout 10m (10m) + the old-ReplicaSet settle deadline
      # (3m) + settle_gateway_404's deadline (2m) + the IAM rollout status --timeout=240s (4m) +
      # the IAM data-path poll, 1 pod in phase B (1.5m) = 10+3+2+4+1.5 = 20.5 minutes.
      # +2.5 min margin.
      - name: helm upgrade (phase B, gateway zone off) and settle
        timeout-minutes: 23
        run: bash ci/kind/run.sh upgrade b
```

- [ ] **Step 4: Check the sums.**

```bash
grep -E '^        timeout-minutes:' .github/workflows/chart.yml | awk '{s += $2} END {print "steps", s}'
grep -E '^    timeout-minutes:' .github/workflows/chart.yml | awk '{print "job", $2}'
```

Expected: `steps 171` and `job 182`. 182 - 171 = 11, the same room as before (169 - 158).

- [ ] **Step 5: The `run.sh` header.** In `ci/kind/run.sh`, replace line 12:

```bash
#   run.sh install a       helm install with values/a.yaml (both zones), then the NOTES check
```

with:

```bash
#   run.sh install a       helm install with values/a.yaml (both zones), then the IAM settle (the
#                          IAM rollout, then IAM /readyz from each console pod) and the NOTES check
```

Replace lines 18-19:

```bash
#   run.sh upgrade b       helm upgrade with a.yaml + b.yaml (gateway zone off), then settle and
#                          the NOTES check
```

with:

```bash
#   run.sh upgrade b       helm upgrade with a.yaml + b.yaml (gateway zone off), then settle, the
#                          IAM settle and the NOTES check
```

- [ ] **Step 6: The README mode table.** In `ci/kind/README.md`, replace the row:

```
| `bash ci/kind/run.sh install a` | `helm install` with `values/a.yaml` (both zones, the CA bundle set), then the NOTES check (SMA-691) |
```

with:

```
| `bash ci/kind/run.sh install a` | `helm install` with `values/a.yaml` (both zones, the CA bundle set), then the IAM settle (SMA-701) and the NOTES check (SMA-691) |
```

and the row:

```
| `bash ci/kind/run.sh upgrade b` | `helm upgrade` with `a.yaml` + `b.yaml` (the gateway zone off), then the settle step and the NOTES check (SMA-691) |
```

with:

```
| `bash ci/kind/run.sh upgrade b` | `helm upgrade` with `a.yaml` + `b.yaml` (the gateway zone off), then the settle step, the IAM settle (SMA-701) and the NOTES check (SMA-691) |
```

- [ ] **Step 7: The README section.** Insert this section directly before the line
`## Reading the evidence`:

````markdown
## The IAM settle (SMA-701)

`install a` and `upgrade b` call `settle_iam_service` before the NOTES check. Spec: `docs/superpowers/specs/2026-10-02-sma-701-kind-iam-service-settle-design.md`.

- **Why `--wait` is not enough.** Helm 3.22.0 (`.prototools`) counts a Deployment as ready when `readyReplicas >= replicas - maxUnavailable` (`pkg/kube/ready.go`). The IAM backend Deployment has `replicas: 1` and `maxUnavailable: 1` on purpose (`charts/paigasus/templates/backend-deployment.yaml`). So the floor is 0, and `helm install --wait` can return while IAM has no Ready pod. Then the IAM Service has no endpoint, the console's discovery probe fails, and the IAM nav link stays disabled (PR 316). Do not remove the settle as a copy of `--wait`.
- **Do not use `kubectl wait --for=condition=Available`.** The Deployment controller sets `Available=True` when `availableReplicas >= replicas - maxUnavailable`, which is 0 here too. `kubectl rollout status` compares `availableReplicas` with `updatedReplicas` and has no such floor. Part A uses it, with a 240 s timeout.
- **Part B** runs `node -e` in each Running console pod (`kubectl exec -c console`). The script reads the IAM URL from `PAIGASUS_SERVICES` and asks `<url>/readyz` until it gets HTTP 200, for up to 60 s per pod. A watchdog stops each `kubectl exec` after 20 s. Each console Deployment of the release must have one Running pod to ask. A pod that is shutting down is not asked.
- **What a run prints.** `iam settle: start HH:MM:SS UTC, IAM readyReplicas=N`. `readyReplicas=0` means that Helm returned before IAM was Ready, so the race occurred and the settle waited. Then `settled: IAM Ready after N s`, the console Deployments it found, and one `settled: <pod> reaches IAM /readyz (HTTP 200)` line per pod.
- **Helm 4.** Helm 4 uses kstatus for `--wait`, which can change this arithmetic. After a Helm 4 bump the settle stays correct, but it can become redundant. Measure it before you remove it.
- **Assumption A1.** The settle helps only if nothing probes IAM through discovery before the settle ends. A failed probe writes a failed record to Redis, and the settle cannot remove it. Today the console `/readyz` routes do not call discovery, the console liveness probe is `tcpSocket`, discovery has no timer and no warm-up, and the first page render is in `specs a`. A change to any of these can make A1 false.
- **Local re-run hazard.** In CI, each run has a new cluster and a new Redis. A local re-run in the same cluster (`helm uninstall`, then `install a`) can find a failed `iam` record from the earlier attempt. Redis keeps it for 600 s (`staleMs`). The settle then passes, and R2 can fail again. Do one of these: render the page two times, wait 600 s, or delete the record. The key is `pgs:svcinfo:iam` (`pgs:svcinfo:<service>`, no prefix in production, `ts/packages/paigasus-discovery/src/adapters/redis-cache.ts`):
  `kubectl --context kind-paigasus-kind -n paigasus-deps exec deploy/redis -- redis-cli DEL pgs:svcinfo:iam`

| Situation | rc | Message start |
| -- | -- | -- |
| The IAM backend Deployment name or object is not found | 2 | `cannot find the IAM backend Deployment` |
| The IAM rollout is not done in 240 s | 1 | `the IAM backend Deployment did not become Ready` |
| No `iam-console` Deployment, or a console Deployment has no Running pod to check | 2 | `no iam-console Deployment` / `no Running <name> pod to check the IAM data path from` |
| At the 60 s deadline the last answer is another HTTP status | 1 | `<pod> reaches IAM, but it answers HTTP <code>` |
| At the deadline there is no answer, and IAM `readyReplicas` is 0 | 1 | `IAM became NotReady after the rollout` |
| At the deadline there is no answer, and IAM is still Ready | 2 | `<pod> cannot reach IAM` + the exec rc and stderr |
| The probe printed something that is not an HTTP status | 2 | `the IAM probe in <pod> printed` |

````

- [ ] **Step 8: The evidence list and "Where to look first".** In the section
`## Reading the evidence`, find:

```
- `stub-check.log` — the stub's in-cluster answer; `gateway-stub.txt` — its Deployment, pods and endpoints
```

Replace it with:

```
- `stub-check.log` — the stub's in-cluster answer; `gateway-stub.txt` — its Deployment, pods and endpoints
- `endpointslices.txt` — every EndpointSlice in `paigasus` (SMA-701)
```

In the list under "Where to look first:", add this bullet after the `helm install --wait` bullet:

```
- `install a` or `upgrade b` fails in the IAM settle (SMA-701): read the `kind:` line, and see the table in "The IAM settle". For `cannot reach IAM`, read `endpointslices.txt` for the `paigasus-paigasus-iam-backend` slice and its ready addresses, then `logs/paigasus-*-iam-backend-*.log`. For `did not become Ready`, read `describe/` for the IAM pod: an image pull error or a failed probe shows there.
```

- [ ] **Step 9: Check the README facts against the code.**

```bash
grep -n 'pgs:svcinfo:' ts/packages/paigasus-discovery/src/adapters/redis-cache.ts
grep -n 'createRedisDescriptorCache(client)' ts/packages/paigasus-console-core/src/discovery.ts
grep -n 'staleMs' ts/packages/paigasus-discovery/src/core/record.ts
grep -n '^helm = ' .prototools
grep -c 'IAM settle' ci/kind/README.md
```

Expected: line 140 holds `` `${prefix}pgs:svcinfo:${service}` ``; `discovery.ts` passes no
`keyPrefix` (so the prefix is `''`); `record.ts` line 38 holds `staleMs: 600_000`;
`.prototools` holds `helm = "3.22.0"`; the README count is 5 or more. If `staleMs` is not
`600_000`, stop and report: the README number then is wrong.

- [ ] **Step 10: actionlint for `chart.yml`.**

First the pinned actionlint alone:

```bash
actionlint .github/workflows/chart.yml; echo "rc=$?"
```

Expected: no output, `rc=0`, in a few seconds. If it does not finish in 60 s, stop it: on a host
whose new pipe holds 512 bytes, actionlint can block on shellcheck's stdin (root `CLAUDE.md`). Then
the full gate below gives the verdict. Then the full gate, under Homebrew bash (it needs bash 5):

```bash
/opt/homebrew/bin/bash ci/actionlint/run.sh >"$SCRATCH/actionlint.log" 2>&1; echo "rc=$?"
grep -n -E 'pipe capacity|FAIL|passed|ABORT|infrastructure' "$SCRATCH/actionlint.log"
```

Read the preflight line first. If it says `pipe capacity 65536 bytes`, expect `rc=0` and no
`FAIL` row; it takes about 50 s. If it says the pipe is small (rc 2 with a `small` message), the
host is in the 512-byte state (root `CLAUDE.md`). Then do not read rc 2 as a result. Record the
preflight line in your report. CI runs `repo:actionlint` on the pull request, because its inputs
are `**/*`. Do not try the Docker route here: the gate needs `uv` and `actionlint` in the
container, and that is a separate setup.

- [ ] **Step 11: Static checks of `run.sh` again.**

```bash
/bin/bash -n ci/kind/run.sh; echo "bash3 rc=$?"
/opt/homebrew/bin/bash -n ci/kind/run.sh; echo "bash5 rc=$?"
uv run --locked --project py shellcheck ci/kind/run.sh; echo "shellcheck rc=$?"
```

Expected: three times `rc=0`.

- [ ] **Step 12: Commit.**

```bash
git add .github/workflows/chart.yml ci/kind/run.sh ci/kind/README.md
git commit -F "$SCRATCH/msg-task3.txt"
```

with `$SCRATCH/msg-task3.txt`:

```
feat(ci): size the kind steps for the IAM settle and document it (SMA-701)

The install a step grows from 12 to 19 minutes, the upgrade b step from
17 to 23, and the job from 169 to 182. The room for the untimed steps
stays 11 minutes. The README explains why helm --wait is not enough,
why kubectl wait for Available has the same defect, assumption A1, and
the local re-run hazard with the Redis key to delete.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

Check: `git show --stat HEAD` lists exactly the three files.

---

### Task 4: static verification of the whole diff

**Files:** none changed, unless a check fails. A fix goes into a NEW commit, never an amend.

- [ ] **Step 1: The diff base.**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-701-kind-iam-settle
git fetch origin main
BASE="$(git merge-base origin/main HEAD)"
git diff --stat "$BASE" HEAD -- ci/ .github/
```

Expected: exactly `.github/workflows/chart.yml`, `ci/kind/README.md` and `ci/kind/run.sh`.

- [ ] **Step 2: Both bash parsers.**

```bash
/bin/bash -n ci/kind/run.sh; echo "bash3 rc=$?"
/opt/homebrew/bin/bash -n ci/kind/run.sh; echo "bash5 rc=$?"
```

Expected: `bash3 rc=0`, `bash5 rc=0`.

- [ ] **Step 3: The banned constructs in the ADDED lines.** `bash -n` accepts `mapfile` and
`declare -A` under 3.2, so it does not check the 3.2 rule.

```bash
git diff "$BASE" HEAD -- ci/kind/run.sh >"$SCRATCH/run.diff"
grep -E '^\+' "$SCRATCH/run.diff" | grep -v '^+++' >"$SCRATCH/added.txt"
wc -l <"$SCRATCH/added.txt"
grep -n -E 'mapfile|declare -A|<<' "$SCRATCH/added.txt"; echo "banned rc=$?"
grep -n -E '\| *(grep -q|grep -m|head|awk|sed)' "$SCRATCH/added.txt"; echo "early-exit rc=$?"
grep -n -E '(^|[^_])timeout ' "$SCRATCH/added.txt"; echo "timeout1 rc=$?"
```

Expected: about 150 added lines; `banned rc=1`, `early-exit rc=1`, `timeout1 rc=1` (grep rc 1
means no match). The pattern `<<` also finds `<<<`.

- [ ] **Step 4: shellcheck against `main`.**

```bash
git show "$BASE:ci/kind/run.sh" >"$SCRATCH/run.main.sh"
uv run --locked --project py shellcheck -f gcc "$SCRATCH/run.main.sh" >"$SCRATCH/sc.main.raw"; echo "main rc=$?"
uv run --locked --project py shellcheck -f gcc ci/kind/run.sh >"$SCRATCH/sc.head.raw"; echo "head rc=$?"
sed 's/^[^:]*:[0-9]*:[0-9]*: //' "$SCRATCH/sc.main.raw" | sort >"$SCRATCH/sc.main.txt"
sed 's/^[^:]*:[0-9]*:[0-9]*: //' "$SCRATCH/sc.head.raw" | sort >"$SCRATCH/sc.head.txt"
diff "$SCRATCH/sc.main.txt" "$SCRATCH/sc.head.txt"; echo "diff rc=$?"
```

Expected: `main rc=0`, `head rc=0`, both files empty, `diff rc=0`. (Measured on 2026-10-02: the
`main` file has no finding.) A new line in `sc.head.txt` is a new finding: fix it in a new commit.

- [ ] **Step 5: The four proofs on the final file.** Re-extract and re-run, because Task 3 edited
`run.sh` again:

```bash
sed -n '/^run_bounded() {/,/^}/p' ci/kind/run.sh >"$SCRATCH/run_bounded.sh"
sed -n "/^PROBE_JS='/,/^});'\$/p" ci/kind/run.sh >"$SCRATCH/probe_js.sh"
/bin/bash "$SCRATCH/run-bounded-proof.sh" "$SCRATCH/run_bounded.sh"; echo "rc=$?"
/opt/homebrew/bin/bash "$SCRATCH/run-bounded-proof.sh" "$SCRATCH/run_bounded.sh"; echo "rc=$?"
/opt/homebrew/bin/bash "$SCRATCH/probe-js-proof.sh" "$SCRATCH/probe_js.sh"; echo "rc=$?"
/bin/bash "$SCRATCH/settle-proof.sh" ci/kind/run.sh "$PWD" fast; echo "rc=$?"
/opt/homebrew/bin/bash "$SCRATCH/settle-proof.sh" ci/kind/run.sh "$PWD" fast; echo "rc=$?"
```

Expected: `rc=0` five times, and `0 failure(s)` on each summary line.

- [ ] **Step 6: The SPDX lines and the timeouts.**

```bash
sed -n 2p ci/kind/run.sh; sed -n 1p .github/workflows/chart.yml; sed -n 1p ci/kind/README.md
grep -n 'timeout-minutes: 19\|timeout-minutes: 23\|timeout-minutes: 182' .github/workflows/chart.yml
grep -c 'settle_iam_service$' ci/kind/run.sh
```

Expected: the three SPDX lines unchanged; three `timeout-minutes` lines; the call count `2`.

- [ ] **Step 7: Report.** Report the outputs of Steps 1-6. Do not commit if nothing changed.

---

### Task 5: the CI red proofs R1, R2, R3 (the coordinator runs this task)

This task pushes a throwaway branch and starts CI runs. Both are outward-facing. The coordinator
confirms with the user before the first push. A subagent does not run this task.

**Files:** `ci/kind/run.sh` on the branch `sma-701-red-proof` only. Never on the feature branch.

- [ ] **Step 1: Make the branch from the feature branch.**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-701-kind-iam-settle
git status --short
git switch -c sma-701-red-proof
```

`git status` may show the uncommitted spec change. It moves with the switch. Do not stage it.

- [ ] **Step 2: R1, a pull failure for the IAM image (Part A, spec §2.1).** In `install_a`,
replace:

```bash
  h install "$RELEASE" "$REPO_ROOT/charts/paigasus" --namespace "$NS" -f "$HERE/values/a.yaml" --wait --timeout 10m \
```

with:

```bash
  h install "$RELEASE" "$REPO_ROOT/charts/paigasus" --namespace "$NS" -f "$HERE/values/a.yaml" --set zones.iam.backend.image.tag=does-not-exist --wait --timeout 10m \
```

Commit and push:

```bash
git add ci/kind/run.sh
git commit -m "test(ci): red proof R1, an IAM image tag that does not exist (SMA-701)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push -u origin sma-701-red-proof
gh workflow run chart.yml --ref sma-701-red-proof
```

- [ ] **Step 3: Find the run and wait for it.**

```bash
git rev-parse HEAD
gh run list --workflow chart.yml --branch sma-701-red-proof --event workflow_dispatch --limit 1 --json databaseId,headSha,status
```

The run can take some seconds to appear. Use the run whose `headSha` equals `git rev-parse HEAD`.
Do not wait with a `sleep` loop in the foreground. Use the Monitor tool with this until-loop, or
run `gh run watch <id> --exit-status` with `run_in_background`:

```bash
until [ "$(gh run view <id> --json status --jq .status)" = completed ]; do sleep 30; done
```

One run stops at `install a`. A full green chart run took about 13 minutes on 2026-10-02 (runs
37054727068 and 36980500352, images included), so a red run takes about 8-12 minutes. A run that
takes more than 30 minutes is not normal: read its log.

- [ ] **Step 4: Read the R1 result.**

```bash
gh run view <id> --json conclusion,jobs --jq '.conclusion, (.jobs[0].steps[] | select(.conclusion == "failure") | .name)'
gh run view <id> --log-failed >"$SCRATCH/r1.log"
grep -n -E 'iam settle|settled:|kind: |exit code|did not become ready in 10 minutes' "$SCRATCH/r1.log"
```

Expected:
- `failure`, and the failed step `helm install (phase A, both zones)`.
- A line `iam settle: start <time> UTC, IAM readyReplicas=0`. This shows that `helm install --wait`
  returned 0 with no Ready IAM pod (spec §2.1).
- `kind: assertion failed (rc=1): the IAM backend Deployment did not become Ready in 240 s`, then
  `Process completed with exit code 1.`
- NO line `helm install with values/a.yaml did not become ready in 10 minutes`.

If Helm itself fails (the last item is present), §2.1 is not measured. Stop and report to the user.

- [ ] **Step 5: R2, a closed port (Part B, rc 2).** Undo R1 and apply R2 in one commit. In
`install_a`, remove ` --set zones.iam.backend.image.tag=does-not-exist` again (the exact reverse of
Step 2). In `PROBE_JS`, replace:

```
  return fetch(iam + "/readyz", { signal: AbortSignal.timeout(5000) });
```

with:

```
  return fetch(iam.replace(":8080", ":8081") + "/readyz", { signal: AbortSignal.timeout(5000) });
```

Then:

```bash
git diff --stat
git add ci/kind/run.sh
git commit -m "test(ci): red proof R2, the IAM probe on a closed port (SMA-701)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push
gh workflow run chart.yml --ref sma-701-red-proof
```

Start R2 only after R1 is `completed`. Find and wait as in Step 3. Then:

```bash
gh run view <id> --log-failed >"$SCRATCH/r2.log"
grep -n -E 'iam settle|settled:|kind: |exit code' "$SCRATCH/r2.log"
gh run download <id> -n kind-evidence -D "$SCRATCH/r2-evidence"
grep -n 'iam-backend' "$SCRATCH/r2-evidence/endpointslices.txt"
```

Expected: the failed step `helm install (phase A, both zones)`; `settled: IAM Ready after N s`;
`console Deployments: gateway-console iam-console`; then
`kind: infrastructure error (rc=2): paigasus-paigasus-gateway-console-<hash> cannot reach IAM /readyz through its Service in 60 s (last kubectl exec rc 0; …)`
about 60 s after the `settled: IAM Ready` line; `Process completed with exit code 2.`
`endpointslices.txt` exists and lists a `paigasus-paigasus-iam-backend-…` slice with the ports
8080 and 9090. This proves the `diagnose` change.

- [ ] **Step 6: R3, a wrong path (Part B, rc 1).** Undo R2 and apply R3 in one commit. In
`PROBE_JS`, the line becomes:

```
  return fetch(iam + "/nope", { signal: AbortSignal.timeout(5000) });
```

```bash
git diff --stat
git add ci/kind/run.sh
git commit -m "test(ci): red proof R3, the IAM probe on a path that does not exist (SMA-701)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push
gh workflow run chart.yml --ref sma-701-red-proof
```

Start R3 only after R2 is `completed`. Then:

```bash
gh run view <id> --log-failed >"$SCRATCH/r3.log"
grep -n -E 'iam settle|settled:|kind: |exit code' "$SCRATCH/r3.log"
```

Expected: `kind: assertion failed (rc=1): paigasus-paigasus-gateway-console-<hash> reaches IAM, but it answers HTTP 404 on /readyz 60 s after the IAM rollout; want 200`
and `Process completed with exit code 1.` IAM's 404 fallback is outside the bearer layer
(`rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` lines 980-982), so 404 is the expected
code. The message says `/readyz` because it names the normal path, not the mutated one. A
different 4xx is still the rc 1 class; record the code that you see.

- [ ] **Step 7: Delete the branch.**

```bash
git switch feature/sma-701-kind-iam-settle
git push origin --delete sma-701-red-proof
git branch -D sma-701-red-proof
git log --oneline "$(git merge-base origin/main HEAD)"..HEAD
```

Expected: the log shows no `red proof` commit. `git ls-remote --heads origin sma-701-red-proof`
prints nothing.

- [ ] **Step 8: Record.** Keep, for the PR body: the three run URLs, the rc and the message line
of each, and the R1 `readyReplicas=0` line.

---

### Task 6: five green runs on the feature branch (the coordinator runs this task)

This task pushes the feature branch and starts CI runs. The coordinator confirms with the user
before the push. Before the push, the coordinator runs the full gate graph from the root
`CLAUDE.md` ("Before you push").

- [ ] **Step 1: Push the feature branch.**

```bash
git push -u origin feature/sma-701-kind-iam-settle
```

- [ ] **Step 2: Five runs, one after the other.** For N = 1 to 5:

```bash
gh workflow run chart.yml --ref feature/sma-701-kind-iam-settle
gh run list --workflow chart.yml --branch feature/sma-701-kind-iam-settle --event workflow_dispatch --limit 1 --json databaseId,headSha,status
```

Wait with Monitor (`until [ "$(gh run view <id> --json status --jq .status)" = completed ]; do sleep 30; done`)
before you start the next run. The concurrency group (`chart.yml` lines 49-52) keeps one pending
run and cancels an older pending one, so parallel dispatches do not give five runs. If a PR is open,
its `pull_request` runs are in a different group and do not cancel these. All five runs must use
the same `headSha`.

- [ ] **Step 3: Each run must pass.**

```bash
gh run view <id> --json conclusion --jq .conclusion
```

Expected: `success` five times. A failure stops the task: keep the run URL and its
`kind-evidence` artifact, and report. Do not re-run a failed run to get a green.

- [ ] **Step 4: Extract the evidence lines.**

```bash
gh run view <id> --log >"$SCRATCH/green-<N>.log"
grep -E 'iam settle: start|settled: IAM Ready|iam settle: console Deployments|reaches IAM /readyz' "$SCRATCH/green-<N>.log"
```

Expected per run: two `iam settle: start` lines (install a, upgrade b); two `settled: IAM Ready`
lines; in `install a`, `console Deployments: gateway-console iam-console` and two `reaches IAM`
lines, one for a `gateway-console` pod and one for an `iam-console` pod; in `upgrade b`,
`console Deployments: iam-console` and exactly one `reaches IAM` line, for an `iam-console` pod.
Any other count is a defect (Review Focus 1, 2 and 3), even when the run is green.

- [ ] **Step 5: The watchdog does not hold the step.** In `green-1.log`, compare the time stamp of
the `== install a: done ==` line with the first line of the next step (`Specs, phase A`). Expected:
less than 5 s apart. A gap near 20 s means that a watchdog `sleep` held the step's output
(Review Focus 4).

- [ ] **Step 6: The table for the PR body.**

```
| Run | install a: readyReplicas at start | install a: IAM Ready after | install a: gateway-console pod / iam-console pod | upgrade b: readyReplicas at start | upgrade b: IAM Ready after | upgrade b: iam-console pod |
|---|---|---|---|---|---|---|
| <url 1> | 0 or 1 | N s | M s / M s | 1 | N s | M s |
```

Use the "for this pod" seconds for the pod columns. Below the table, state how many of the five
runs had `readyReplicas=0` in `install a`. That count shows whether the race occurred. State the
limit from spec §6: five green runs make the fix probable; they do not prove it. R1 shows that Helm
returns before IAM is Ready and that the helper detects it.

---

## Spec coverage

| Spec item | Task |
|---|---|
| §0 D1 no `probeService` change, D2 two call sites, D3 rollout status + exec poll | Global Constraints; Task 2 Steps 3-4 |
| §2.1 Helm arithmetic, `kubectl wait` defect | Task 2 comment; Task 3 Step 7; Task 5 R1 |
| §2.2 cache model | Task 3 Step 7 (A1, re-run hazard) |
| §2.3 A1 and the local re-run hazard with remedies and the Redis key | Task 3 Steps 7 and 9 |
| §2.4 the entry line measures the race; Part B covers kube-proxy | Task 2 Step 3; Task 5 R1; Task 6 |
| §3 G1-G5 | Task 2 (G1, G2, G4, G5); Task 2 Step 4 (G3); Task 6 (G5 evidence) |
| §4.1 entry, Part A, Part B 1-4, PROBE_JS, shell rules, pods | Task 1; Task 2 Steps 1-7 |
| §4.2 call sites | Task 2 Step 4 |
| §4.3 timeouts 19 / 23 / 182, comments | Task 3 Steps 1-4 |
| §4.4 header, README table and note, `diagnose` EndpointSlices | Task 2 Step 4; Task 3 Steps 5-8 |
| §5 failure classes | Task 2 Step 3; harness rows S2, S4-S7, D1-D6; Task 3 Step 7 table |
| §6.1 static checks | Tasks 1-4 static steps; Task 3 Step 10 |
| §6.2 the watchdog proof, both bash versions | Task 1 Steps 4-5 |
| §6.3 red proofs R1-R3 | Task 5 |
| §6.4 five sequential green runs and the table | Task 6 |
| §7 R1 exec into distroless, R2 UID 65532 | Task 5 R3; Task 6 |
| §7 R3 wall time, R4 Helm 4, R5 JWKS | Task 6 times; Task 3 Step 7 (Helm 4); R5 out of scope |
| §8 rejected alternatives | not built |
