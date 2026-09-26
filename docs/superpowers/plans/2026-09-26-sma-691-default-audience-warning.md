<!-- moon-diagnosis:ok -->
# SMA-691 Default IAM Audience Warning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When the IAM audience equals `oidc.clientId` (the chart default), the chart shows a warning in the release NOTES and in an annotation on the IAM backend Deployment. The optional value `oidc.acknowledgeClientIdAudience` removes the warning. The chart does not change what IAM accepts.

**Architecture:** A new helper file `charts/paigasus/templates/_audience.tpl` holds three helpers: `paigasus.iamAudience` (the audience string, used by `IAM_AUTHN__ISSUERS`), `paigasus.iamAudienceWarns` (the one condition), and `paigasus.iamAudienceNotes` (the NOTES body). A new `templates/NOTES.txt` only includes the NOTES helper. The IAM backend Deployment gets the annotation `paigasus.io/iam-audience-warning` in `metadata.annotations`. `charts/paigasus/tests/env.sh` gets rows W1-W14 (the annotation) and N0-N5 (the NOTES text, through a probe chart). The kind job asserts the NOTES marker after `install a` and after `upgrade b`. The runbook recommends a dedicated API audience and gives the migration order.

**Tech Stack:** Helm 3.22.0 (proto-pinned, `v3.22.0+g144ca65`), Go templates with sprig, bash (`env.sh` and `ci/kind/run.sh` must run under `/bin/bash` 3.2.57 and bash 5), Python 3 with PyYAML (inline in the chart scripts), Moon 2.5.3 through proto shims.

**Spec:** docs/superpowers/specs/2026-09-26-sma-691-default-audience-warning-design.md

## Global Constraints

- Worktree: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience`, branch `feature/sma-691-chart-default-audience`. A subagent starts in the MAIN checkout. It must `cd` to the worktree and run `git branch --show-current` first. The output must be `feature/sma-691-chart-default-audience`.
- Every command block starts with `cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience` and `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Run every `helm` and chart-script command from the worktree root. Outside the repository the proto shim finds no `.prototools` and runs the global helm (v4.3.0 on this Mac). The goldens then differ for a reason that is not this change.
- The chart scripts call `python3`, which must import `yaml`. Check with `python3 -c 'import yaml'` (measured: PyYAML 6.0.3 at `/opt/homebrew/bin/python3`).
- In this sandbox a Bash call with a shell loop or a variable can be refused. If so, write the commands to a script file in your session scratchpad and run it with `/bin/bash <script>`.
- Every new source file opens with an SPDX header: `{{/* SPDX-License-Identifier: Apache-2.0 */}}` for `_audience.tpl`, and `{{- /* SPDX-License-Identifier: Apache-2.0 */ -}}` as line 1 of `NOTES.txt` (trimmed, so an install with no warning prints no NOTES). The new measurements file follows the existing files in `docs/superpowers/specs/`, which have no SPDX line.
- `charts/paigasus/tests/env.sh` runs today under `/bin/bash` 3.2.57 AND under bash 5 (`repo:helm-render` runs each chart script as `"$BASH" <script>`, and CI's Linux bash is 5). Keep both working: no `mapfile`, no `declare -A`, no here-string, no here-doc, no pipe into a reader that exits early. Keep the `"${BASE[@]+"${BASE[@]}"}"` guard. Inside a `python3 -c '…'` block use only DOUBLE quotes. Write an apostrophe as `chr(39)`.
- `ci/kind/run.sh` has the same rules (its header, lines 27-30). No pipe into an early-exit reader: `repo:actionlint` check 13 scans every tracked `*.sh`, this file included. Capture into a variable, then match with `case`.
- Renders in the new rows go to FILES under `$TMP`, never through a pipe. A new pipe on this Mac can hold only 512 bytes (root `CLAUDE.md`, SMA-612).
- The golden-file rule: regenerate `charts/paigasus/tests/golden/*.yaml` only in Task 2, only with `bash charts/paigasus/tests/render.sh --update`, and then check the diff. The ONLY change is two added lines per file (`  annotations:` and the annotation line). `git diff --numstat charts/paigasus/tests/golden` must print `2	0` for each of the two files. Any other diff is a defect: stop and report.
- The annotation key is exactly `paigasus.io/iam-audience-warning`. Its value is exactly this ASCII string (no `§`): `the IAM audience equals oidc.clientId, so an ID token passes IAM's audience check. See docs/ops/RUNBOOK-chart.md section 6 (SMA-691).`
- The annotation is in the IAM backend Deployment's `metadata.annotations`, NEVER in `spec.template.metadata.annotations`. Its explanation goes only in a Go template comment `{{- /* … */}}`, never in a YAML comment (a YAML comment renders into both goldens).
- The exact content of `charts/paigasus/templates/NOTES.txt` is these three lines, each ending with a newline:
  `{{- /* SPDX-License-Identifier: Apache-2.0 */ -}}`
  `{{- include "paigasus.validate" . -}}`
  `{{- include "paigasus.iamAudienceNotes" . -}}`
- The exact NOTES body (spec § 4.3), with `<audience>` as the output of `paigasus.iamAudience` in double quotes:
  ```text
  WARNING (SMA-691): the IAM audience equals oidc.clientId, so an ID token passes IAM's audience check.
  IAM accepts the audience "<audience>". An OIDC ID token has the client id as its audience.
  IAM refuses a Keycloak ID token by its typ claim (SMA-686). Dex does not set that claim.
  Other IdPs are not measured.
  Recommended: give the API its own audience and set oidc.audience to it.
  Follow the order in docs/ops/RUNBOOK-chart.md section 6, or every session breaks.
  If your IdP cannot do this (Dex), set oidc.acknowledgeClientIdAudience to the value of
  oidc.clientId to remove this warning.
  ```
- The kind marker line is exactly `WARNING (SMA-691): the IAM audience equals oidc.clientId`.
- The new value is `oidc.acknowledgeClientIdAudience` (string, default `""`, NOT required). It acknowledges only when `toString` of it equals `toString .Values.oidc.clientId`. Because it is not required, do NOT change the seven chart scripts' value lists, `ci/helm-render/helm_render.py` `STUB_VALUES`, or `ci/kind/values/a.yaml` / `b.yaml`.
- Do NOT change `charts/paigasus/templates/_helpers.tpl` (spec D5). Its two whole-file copies under `ci/helm-render/fixtures/` then stay valid.
- The render of `IAM_AUTHN__ISSUERS` stays byte-identical for every input (spec D1). Rows A1-A6 and the goldens prove it.
- Commit scopes: `repo` for the chart and the docs, `ci` for `ci/kind/`. `charts` is NOT a valid scope (`ts/packages/commitlint-config/index.cjs`). SMA-678 used `feat(repo): …`; SMA-514 used `feat(ci): …`.
- Commit subjects are lowercase after the scope (commitlint `subject-case` refuses sentence case). Header max 100 characters. Body lines max 100 characters.
- No commit body line starts with `#` followed by a number. No body line has the form `Word: value`. Only the trailer has that form. Such lines fail `footer-leading-blank`.
- Every commit message ends with one blank line and then `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Never use `--no-verify`. If `commitlint` is not found, provision the worktree (`proto install`, then `pnpm -C ts install`) and commit again.
- Never use `git commit --amend`. Never use `git reset`. Never use `git stash`. Each task makes one new commit.
- Remove every mutation with the Edit tool (the inverse Edit). Never use `git checkout --` or `git restore`: they also discard the uncommitted change under test.
- Stage exact paths with `git add <path>`. Never `git add -A` or `git add .`.
- If the sandbox refuses the HEREDOC commit form, write the message to a file in your session scratchpad and run `git commit -F <that file>`.
- Do not install software (no `brew`, no `pip install`). If a tool is missing, stop and report.
- Write every new comment, doc text and commit message in ASD-STE100 Simplified Technical English: short sentences, active voice, no idiom. Keep technical names as they are.

## Review Focus

1. **A number as the acknowledgement, from a values file.** A values file gives a float64. `toString` of `12345.0` is `12345`, but `toString` of `1000000.0` is `1e+06` (MEASURED). A large unquoted number therefore never acknowledges, and the warning stays (it fails closed). Task 2 adds rows `W10 number-in-file` (absent) and `W11 large-number-in-file` (present). Mutation M13 (compare without `toString`) must red W6, W10 and W11. Task 5 tells the operator to quote the value.
2. **The letter case of the acknowledgement.** An operator can type `PAIGASUS-CONSOLE` for `paigasus-console`. The compare must be exact. Task 2 adds row `W12 case-differs` (present). Mutation M12 (a `lower` compare) must red W12.
3. **The gateway zone on.** W1 renders with the gateway zone off. With the zone on, the render has three Deployments, and the key must still occur exactly once and only on the IAM backend. Task 2 adds row `W13 gateway-on`. Note: no valid render has a second BACKEND Deployment (`paigasus.validate` refuses `zones.gateway.backend.deploy=true` with the zone on), so mutation M5 (drop `eq $id "iam"`) is an EQUIVALENT mutant. MEASURED: no row reds. See the flag in Notes.
4. **The acknowledgement must not restart a pod.** The runbook (Task 5, § 5 table) says that the value restarts nothing. Task 2 adds row `W14 no-restart`: the renders with and without the acknowledgement differ in the IAM Deployment metadata and have equal `spec.template` for all three Deployments. Mutation M14 (the value fed into a pod checksum) must red W14.
5. **bash 3.2 and bash 5.** A local run of `env.sh` can use `/bin/bash` 3.2.57 or Homebrew bash 5.3.15, and CI uses Linux bash 5. `ci/kind/run.sh` must run under both too (its header). Tasks 2, 3 and 4 run their new code under `/bin/bash` AND `/opt/homebrew/bin/bash`. Task 4 runs the kind NOTES harness under both.

---

### Task 1: The `paigasus.iamAudience` helper and the refactor of `IAM_AUTHN__ISSUERS`

This task is a pure refactor. Its test is the existing rows A1-A6 and the unchanged goldens (byte identity, spec § 5.3), and a mutation that proves A1-A6 now read through the helper.

**Files:**
- Create: `charts/paigasus/templates/_audience.tpl`
- Modify: `charts/paigasus/templates/backend-deployment.yaml:114`
- Test: `charts/paigasus/tests/env.sh` (rows A1-A6, unchanged), `charts/paigasus/tests/render.sh` (goldens, unchanged)

**Interfaces:**
- Consumes: nothing.
- Produces: `include "paigasus.iamAudience" <root context>` returns a string: `.Values.oidc.audience | default .Values.oidc.clientId | toString`. It takes the ROOT context. Inside the zone `range` of `backend-deployment.yaml` pass `$root`, never `.`.

- [ ] **Step 1: Run the baseline and confirm it is green**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git branch --show-current
/bin/bash charts/paigasus/tests/env.sh --set ingress.host=console.example.test; echo "env rc=$?"
/bin/bash charts/paigasus/tests/render.sh; echo "render rc=$?"
```

Expected: `feature/sma-691-chart-default-audience`; env.sh prints `ok` for `iam only`, `iam and gateway`, `A1 unset` … `A6 restart-scope`, then `== chart env OK ==` and `env rc=0`; render.sh prints `ok [iam-only]`, `ok [iam-and-gateway]`, `== chart render OK ==`, `render rc=0`.

- [ ] **Step 2: Create the helper file**

Create `charts/paigasus/templates/_audience.tpl` with exactly this content:

```gotemplate
{{/* SPDX-License-Identifier: Apache-2.0 */}}

{{/*
The IAM access-token audience (SMA-678, SMA-691). Each helper in this file takes the ROOT context.
A caller inside the zone range of backend-deployment.yaml passes $root, never "." (inside that
range, "." is the zone map).
*/}}

{{/*
paigasus.iamAudience: the one audience that IAM accepts, as a string. An empty or absent (nil)
oidc.audience gives oidc.clientId. A set value REPLACES oidc.clientId (SMA-678). toString comes
from SMA-678. MEASURED (SMA-691): include already prints a number as its digits, so toString
changes no render here.
*/}}
{{- define "paigasus.iamAudience" -}}
{{- .Values.oidc.audience | default .Values.oidc.clientId | toString -}}
{{- end -}}
```

- [ ] **Step 3: Use the helper in `IAM_AUTHN__ISSUERS`**

Use the Edit tool on `charts/paigasus/templates/backend-deployment.yaml`.

old_string:
```text
              value: {{ printf "[{issuer=%q,audiences=[%q]}]" $root.Values.oidc.issuer ($root.Values.oidc.audience | default $root.Values.oidc.clientId | toString) | quote }}
```

new_string:
```text
              value: {{ printf "[{issuer=%q,audiences=[%q]}]" $root.Values.oidc.issuer (include "paigasus.iamAudience" $root) | quote }}
```

Do not change the SMA-678 Go template comment (lines 100-109) or the conditional YAML comment (lines 110-112).

- [ ] **Step 4: Run the rows and the goldens, and expect them to pass unchanged**

Run the Step 1 commands again.

Expected: the same output as Step 1. `render rc=0` proves byte identity of the whole render for the two golden inputs. A1-A6 prove the exact `IAM_AUTHN__ISSUERS` value for unset, nil, set, int64 and float64 inputs.

- [ ] **Step 5: Prove that A1 and A2 read through the helper (mutation T1-M)**

Use the Edit tool on `charts/paigasus/templates/_audience.tpl`:
old_string `{{- .Values.oidc.audience | default .Values.oidc.clientId | toString -}}`
new_string `{{- .Values.oidc.audience | toString -}}`

Run `/bin/bash charts/paigasus/tests/env.sh --set ingress.host=console.example.test; echo "env rc=$?"`.

Expected (MEASURED on a scratch copy): `FAIL [A1 unset]: IAM_AUTHN__ISSUERS is '[{issuer="https://idp.example.test/realms/paigasus",audiences=[""]}]', …` and `FAIL [A2 reuse-values-no-key]: … audiences=["<nil>"] …`, `env rc=1`.

Remove the mutation with the inverse Edit (old_string `{{- .Values.oidc.audience | toString -}}`, new_string `{{- .Values.oidc.audience | default .Values.oidc.clientId | toString -}}`). Run Step 1 again: all green.

- [ ] **Step 6: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
git add charts/paigasus/templates/_audience.tpl charts/paigasus/templates/backend-deployment.yaml
git status --short
git commit -m "$(cat <<'EOF'
refactor(repo): read the IAM audience through the paigasus.iamAudience helper (SMA-691)

IAM_AUTHN__ISSUERS now takes its audience from a named helper in the new file
templates/_audience.tpl. The render is byte-identical: rows A1-A6 and both golden
files pass unchanged. The SMA-691 warning helpers use the same helper in the next
commits, so the condition and the rendered audience cannot differ.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
git log --oneline -1
```

Expected: `git status --short` lists `A  charts/paigasus/templates/_audience.tpl` and `M  charts/paigasus/templates/backend-deployment.yaml` only.

---

### Task 2: The value, `paigasus.iamAudienceWarns`, the annotation, rows W1-W14 and the goldens

**Files:**
- Modify: `charts/paigasus/tests/env.sh` (header after line 32; tail at lines 199-204)
- Modify: `charts/paigasus/values.yaml` (after line 87, inside the `oidc` block)
- Modify: `charts/paigasus/templates/_audience.tpl` (append)
- Modify: `charts/paigasus/templates/backend-deployment.yaml:9-11`
- Modify: `charts/paigasus/tests/golden/iam-only.yaml`, `charts/paigasus/tests/golden/iam-and-gateway.yaml` (regenerated)

**Interfaces:**
- Consumes: `include "paigasus.iamAudience" <root>` (Task 1).
- Produces: `include "paigasus.iamAudienceWarns" <root>` returns the string `true` when the IAM audience equals `toString .Values.oidc.clientId` AND `toString .Values.oidc.acknowledgeClientIdAudience` does not equal it; otherwise `""`. New value `oidc.acknowledgeClientIdAudience` (string, default `""`). New `env.sh` functions `check_warning <label> <present|absent|pod-template> [helm args...]` and `check_warning_restart <label>`, counter `WARNING_ROWS` / `WARNING_ROWS_WANT=14`.

- [ ] **Step 1: Write the failing rows in env.sh (header)**

Use the Edit tool on `charts/paigasus/tests/env.sh`.

old_string:
```text
#   A6 restart-scope
#                   A change of the value changes the IAM pod template. It does not change a
#                   console pod template.
# A row counter reds the script when a row call line is deleted.
set -euo pipefail
```

new_string:
```text
#   A6 restart-scope
#                   A change of the value changes the IAM pod template. It does not change a
#                   console pod template.
# A row counter reds the script when a row call line is deleted.
#
# The script also checks the SMA-691 warning annotation paigasus.io/iam-audience-warning on the
# IAM backend Deployment (check_warning, check_warning_restart). One row per input:
#   W1 default      The annotation is present. The key occurs once in the whole render.
#   W2 reuse-values-no-key
#                   `--set oidc.acknowledgeClientIdAudience=null`. The template reads nil. Present.
#   W3 explicit-equal
#                   oidc.audience is set to the client id. Present.
#   W4 distinct     oidc.audience differs from the client id. Absent.
#   W5 acknowledged The acknowledgement equals the client id. Absent.
#   W6 bool-true    The acknowledgement is the bool true. Present.
#   W7 string-false The acknowledgement is the string "false". Present.
#   W8 stale-ack    The acknowledgement names another client id. Present.
#   W9 pod-template The key is not in spec.template.metadata.annotations.
#   W10 number-in-file
#                   The acknowledgement 12345 from a values file (a float64) equals the client id
#                   12345. Absent.
#   W11 large-number-in-file
#                   The acknowledgement 1000000 from a values file renders as "1e+06". It does not
#                   equal the client id "1000000". Present.
#   W12 case-differs
#                   The compare is exact. PAIGASUS-CONSOLE does not acknowledge paigasus-console.
#   W13 gateway-on  The gateway zone is on. The key still occurs once.
#   W14 no-restart  The acknowledgement changes no pod template, so it restarts no pod.
# A second row counter reds the script when a W row call line is deleted.
set -euo pipefail
```

- [ ] **Step 2: Write the failing rows in env.sh (functions and rows)**

Use the Edit tool on `charts/paigasus/tests/env.sh`.

old_string:
```text
if [ "$AUDIENCE_ROWS" -lt "$AUDIENCE_ROWS_WANT" ]; then
  echo "FAIL [audience rows]: $AUDIENCE_ROWS audience row(s) ran, want $AUDIENCE_ROWS_WANT"; ec=1
fi

if [ "$ec" -eq 0 ]; then echo "== chart env OK =="; fi
```

new_string:
```bash
if [ "$AUDIENCE_ROWS" -lt "$AUDIENCE_ROWS_WANT" ]; then
  echo "FAIL [audience rows]: $AUDIENCE_ROWS audience row(s) ran, want $AUDIENCE_ROWS_WANT"; ec=1
fi

# The IAM audience warning annotation (SMA-691). Renders go to a file, as for check_audience.
WARNING_ROWS=0
WARNING_ROWS_WANT=14

# check_warning <label> <present|absent|pod-template> [helm args...]
#   present       The IAM backend Deployment's metadata.annotations holds the key with the exact
#                 value. The key occurs exactly once in the whole render.
#   absent        The IAM backend Deployment exists. The key occurs nowhere in the render.
#   pod-template  The IAM backend Deployment exists. Its spec.template.metadata.annotations does
#                 not hold the key. A key there restarts the pod on every change.
check_warning() {
  local label="$1" mode="$2"; shift 2
  local out
  WARNING_ROWS=$((WARNING_ROWS + 1))
  if ! helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" "$@" >"$TMP/warning.yaml" 2>"$TMP/warning.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/warning.err"; ec=1; return 0
  fi
  if ! out="$(MODE="$mode" python3 -c '
import os, sys, yaml
key = "paigasus.io/iam-audience-warning"
value = ("the IAM audience equals oidc.clientId, so an ID token passes IAM" + chr(39)
         + "s audience check. See docs/ops/RUNBOOK-chart.md section 6 (SMA-691).")
with open(sys.argv[1]) as fh:
    raw = fh.read()
docs = [d for d in yaml.safe_load_all(raw) if d]
problems = []
deps = [d for d in docs if d.get("kind") == "Deployment"
        and d["spec"]["template"]["metadata"]["labels"].get("app.kubernetes.io/name") == "iam-backend"]
count = raw.count(key)
mode = os.environ["MODE"]
if len(deps) != 1:
    problems.append(str(len(deps)) + " iam-backend Deployment(s), want 1")
elif mode == "present":
    got = (deps[0]["metadata"].get("annotations") or {}).get(key)
    if got != value:
        problems.append("metadata.annotations[" + key + "] is " + repr(got) + ", want " + repr(value))
    if count != 1:
        problems.append("the key occurs " + str(count) + " time(s) in the render, want 1")
elif mode == "absent":
    if count != 0:
        problems.append("the key occurs " + str(count) + " time(s) in the render, want 0")
elif mode == "pod-template":
    if key in (deps[0]["spec"]["template"]["metadata"].get("annotations") or {}):
        problems.append("spec.template.metadata.annotations holds " + key + ", want it absent")
else:
    problems.append("unknown mode " + repr(mode))
print("|".join(problems) if problems else "OK")' "$TMP/warning.yaml" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

# check_warning_restart <label>
# The acknowledgement changes only Deployment metadata. It does not change a pod template, so it
# restarts no pod. The gateway zone is on, so all three Deployments are in both renders.
check_warning_restart() {
  local label="$1" out
  WARNING_ROWS=$((WARNING_ROWS + 1))
  if ! helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" --set zones.gateway.enabled=true >"$TMP/warn-restart-1.yaml" 2>"$TMP/warn-restart.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/warn-restart.err"; ec=1; return 0
  fi
  if ! helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" --set zones.gateway.enabled=true --set oidc.acknowledgeClientIdAudience=paigasus-console >"$TMP/warn-restart-2.yaml" 2>"$TMP/warn-restart.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/warn-restart.err"; ec=1; return 0
  fi
  if ! out="$(python3 -c '
import sys, yaml
key = "paigasus.io/iam-audience-warning"
def deps(path):
    with open(path) as fh:
        docs = [d for d in yaml.safe_load_all(fh) if d]
    return {d["spec"]["template"]["metadata"]["labels"]["app.kubernetes.io/name"]: d for d in docs if d.get("kind") == "Deployment"}
a, b = deps(sys.argv[1]), deps(sys.argv[2])
want = ["gateway-console", "iam-backend", "iam-console"]
problems = []
if sorted(a) != want or sorted(b) != want:
    problems.append("Deployments are " + repr(sorted(a)) + " and " + repr(sorted(b)) + ", want " + repr(want))
else:
    if key not in (a["iam-backend"]["metadata"].get("annotations") or {}):
        problems.append("the first render has no " + key + ", so the row cannot decide anything")
    if key in (b["iam-backend"]["metadata"].get("annotations") or {}):
        problems.append("the acknowledged render still has " + key)
    problems += [n + ": spec.template differs; it must be equal" for n in want if a[n]["spec"]["template"] != b[n]["spec"]["template"]]
print("|".join(problems) if problems else "OK")' "$TMP/warn-restart-1.yaml" "$TMP/warn-restart-2.yaml" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

# W10 and W11: a number in a VALUES FILE is a float64. --set gives an int64 or a string.
printf 'oidc:\n  acknowledgeClientIdAudience: 12345\n' >"$TMP/ack-number.yaml"
printf 'oidc:\n  acknowledgeClientIdAudience: 1000000\n' >"$TMP/ack-large-number.yaml"

check_warning "W1 default"               present
check_warning "W2 reuse-values-no-key"   present      --set oidc.acknowledgeClientIdAudience=null
check_warning "W3 explicit-equal"        present      --set oidc.audience=paigasus-console
check_warning "W4 distinct"              absent       --set oidc.audience=api://paigasus
check_warning "W5 acknowledged"          absent       --set oidc.acknowledgeClientIdAudience=paigasus-console
check_warning "W6 bool-true"             present      --set oidc.acknowledgeClientIdAudience=true
check_warning "W7 string-false"          present      --set-string oidc.acknowledgeClientIdAudience=false
check_warning "W8 stale-ack"             present      --set oidc.acknowledgeClientIdAudience=old-client
check_warning "W9 pod-template"          pod-template
check_warning "W10 number-in-file"       absent       --set oidc.clientId=12345 -f "$TMP/ack-number.yaml"
check_warning "W11 large-number-in-file" present      --set-string oidc.clientId=1000000 -f "$TMP/ack-large-number.yaml"
check_warning "W12 case-differs"         present      --set oidc.acknowledgeClientIdAudience=PAIGASUS-CONSOLE
check_warning "W13 gateway-on"           present      --set zones.gateway.enabled=true
check_warning_restart "W14 no-restart"

if [ "$WARNING_ROWS" -lt "$WARNING_ROWS_WANT" ]; then
  echo "FAIL [warning rows]: $WARNING_ROWS warning row(s) ran, want $WARNING_ROWS_WANT"; ec=1
fi

if [ "$ec" -eq 0 ]; then echo "== chart env OK =="; fi
```

Why W10 works: helm applies `-f` files first and every `--set` after them, in order. So the later `--set oidc.clientId=12345` replaces BASE's `paigasus-console`.

- [ ] **Step 3: Run the rows and expect them to fail**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash charts/paigasus/tests/env.sh --set ingress.host=console.example.test; echo "env rc=$?"
```

Expected (MEASURED on a scratch copy): `env rc=1`. These rows fail with `metadata.annotations[paigasus.io/iam-audience-warning] is None, want …`: W1, W2, W3, W6, W7, W8, W11, W12, W13. W14 fails with `the first render has no paigasus.io/iam-audience-warning, so the row cannot decide anything`. W4, W5, W9 and W10 pass (an "absent" row passes when nothing adds the key). A1-A6 still pass.

- [ ] **Step 4: Add the value**

Use the Edit tool on `charts/paigasus/values.yaml`.

old_string:
```text
                         # Quote this value in a values file. See docs/ops/RUNBOOK-chart.md § 6.
  existingSecret: ""     # REQUIRED. Must hold keys: oidc-client-secret, session-redis-url.
```

new_string:
```text
                         # Quote this value in a values file. See docs/ops/RUNBOOK-chart.md § 6.
  acknowledgeClientIdAudience: ""   # NOT required. Set it to the value of oidc.clientId to
                                    # remove the warning (NOTES and the paigasus.io/iam-audience-
                                    # warning annotation) that shows when the IAM audience equals
                                    # oidc.clientId. It does not change what IAM accepts. A
                                    # change of oidc.clientId shows the warning again. Quote the
                                    # value in a values file. See RUNBOOK-chart.md § 6.
  existingSecret: ""     # REQUIRED. Must hold keys: oidc-client-secret, session-redis-url.
```

(Task 5 rewrites the `oidc.audience` comment above this key. Do not change it here.)

- [ ] **Step 5: Append `paigasus.iamAudienceWarns` to the helper file**

Use the Edit tool on `charts/paigasus/templates/_audience.tpl`.

old_string:
```text
{{- define "paigasus.iamAudience" -}}
{{- .Values.oidc.audience | default .Values.oidc.clientId | toString -}}
{{- end -}}
```

new_string:
```gotemplate
{{- define "paigasus.iamAudience" -}}
{{- .Values.oidc.audience | default .Values.oidc.clientId | toString -}}
{{- end -}}

{{/*
paigasus.iamAudienceWarns: "true" when both conditions are true, else "".
  1. The IAM audience (paigasus.iamAudience) equals oidc.clientId. An OIDC ID token has the
     client id as its aud, so an ID token then passes IAM's audience check.
  2. oidc.acknowledgeClientIdAudience does not equal oidc.clientId. The compare is exact, as
     strings. A nil value (helm upgrade --reuse-values from a release made before the key
     existed) gives "<nil>" from toString, so the warning shows.
There is no "the IAM backend is deployed" condition: paigasus.validate refuses every render
without the IAM backend. Both signals (NOTES.txt and the paigasus.io/iam-audience-warning
annotation) call this helper. No other file repeats the condition.
*/}}
{{- define "paigasus.iamAudienceWarns" -}}
{{- $clientId := toString .Values.oidc.clientId -}}
{{- if and (eq (include "paigasus.iamAudience" .) $clientId) (ne (toString .Values.oidc.acknowledgeClientIdAudience) $clientId) -}}
true
{{- end -}}
{{- end -}}
```

- [ ] **Step 6: Add the annotation to the IAM backend Deployment metadata**

Use the Edit tool on `charts/paigasus/templates/backend-deployment.yaml`.

old_string:
```text
metadata:
  name: {{ include "paigasus.name" (list $root (printf "%s-backend" $id)) }}
spec:
```

new_string:
```gotemplate
metadata:
  name: {{ include "paigasus.name" (list $root (printf "%s-backend" $id)) }}
{{- if and (eq $id "iam") (include "paigasus.iamAudienceWarns" $root) }}
{{- /*
  SMA-691: the second signal, for a user of `helm template` who never sees NOTES.txt. It is on
  the Deployment metadata, not on the pod template, so it does not restart a pod. This is a Go
  template comment on purpose: the condition is true by default, so a YAML comment here renders
  into both golden files.
*/}}
  annotations:
    paigasus.io/iam-audience-warning: "the IAM audience equals oidc.clientId, so an ID token passes IAM's audience check. See docs/ops/RUNBOOK-chart.md section 6 (SMA-691)."
{{- end }}
spec:
```

The comment must start with `{{- /*` (left trim). MEASURED: `{{/*` adds one blank line; env.sh stays green and only the goldens catch it (mutation M10, Task 6).

- [ ] **Step 7: Run env.sh and expect it to pass; run render.sh and expect exactly the golden diff**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash charts/paigasus/tests/env.sh --set ingress.host=console.example.test; echo "env rc=$?"
/bin/bash charts/paigasus/tests/render.sh; echo "render rc=$?"
```

Expected: env.sh prints `ok` for every row through `W14 no-restart`, then `== chart env OK ==`, `env rc=0`. render.sh prints a unified diff that ADDS exactly these two lines after `  name: paigasus-paigasus-iam-backend` in each golden file, then `FAIL [iam-only]`, `FAIL [iam-and-gateway]` and `render rc=1`:

```text
+  annotations:
+    paigasus.io/iam-audience-warning: "the IAM audience equals oidc.clientId, so an ID token passes IAM's audience check. See docs/ops/RUNBOOK-chart.md section 6 (SMA-691)."
```

- [ ] **Step 8: Regenerate the goldens and check the diff**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash charts/paigasus/tests/render.sh --update
git diff --numstat charts/paigasus/tests/golden
git diff charts/paigasus/tests/golden
/bin/bash charts/paigasus/tests/render.sh; echo "render rc=$?"
```

Expected (MEASURED on a scratch copy): `updated iam-only.yaml`, `updated iam-and-gateway.yaml`; numstat `2	0	charts/paigasus/tests/golden/iam-and-gateway.yaml` and `2	0	charts/paigasus/tests/golden/iam-only.yaml`; the diff adds the two lines of Step 7 after line 70 of `iam-only.yaml` and after line 85 of `iam-and-gateway.yaml`, and nothing else; then `ok [iam-only]`, `ok [iam-and-gateway]`, `== chart render OK ==`, `render rc=0`. If numstat shows anything other than `2	0` per file, stop and report.

- [ ] **Step 9: Run env.sh under both bashes**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash charts/paigasus/tests/env.sh --set ingress.host=console.example.test; echo "bash3 rc=$?"
/opt/homebrew/bin/bash charts/paigasus/tests/env.sh --set ingress.host=console.example.test; echo "bash5 rc=$?"
/bin/bash charts/paigasus/tests/ca-bundle.sh --set ingress.host=console.example.test; echo "ca rc=$?"
```

Expected: `bash3 rc=0`, `bash5 rc=0` (MEASURED on a scratch copy: both pass in about 1 s), `== chart ca-bundle OK ==`, `ca rc=0`. If the Homebrew bash run hangs at about 0% CPU, stop it: that is the host pipe condition (SMA-612), not this change. Record it and rely on the `/bin/bash` result.

- [ ] **Step 10: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
git add charts/paigasus/values.yaml charts/paigasus/templates/_audience.tpl charts/paigasus/templates/backend-deployment.yaml charts/paigasus/tests/env.sh charts/paigasus/tests/golden/iam-only.yaml charts/paigasus/tests/golden/iam-and-gateway.yaml
git status --short
git commit -m "$(cat <<'EOF'
feat(repo): annotate the IAM Deployment when the audience equals the client id (SMA-691)

When the IAM audience equals oidc.clientId, an ID token passes IAM's audience
check. The IAM backend Deployment then gets the metadata annotation
paigasus.io/iam-audience-warning, for users of helm template. It is not on the
pod template, so it restarts no pod. The new optional value
oidc.acknowledgeClientIdAudience, set to the client id, removes it. The chart
does not change what IAM accepts. env.sh adds rows W1-W14 and a row counter.
Both golden files gain the two annotation lines and nothing else.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
git log --oneline -1
```

---

### Task 3: `paigasus.iamAudienceNotes`, `templates/NOTES.txt` and rows N0-N5

**Files:**
- Modify: `charts/paigasus/tests/env.sh` (header; tail)
- Modify: `charts/paigasus/templates/_audience.tpl` (append)
- Create: `charts/paigasus/templates/NOTES.txt`

**Interfaces:**
- Consumes: `include "paigasus.iamAudienceWarns" <root>` and `include "paigasus.iamAudience" <root>` (Tasks 1, 2).
- Produces: `include "paigasus.iamAudienceNotes" <root>` returns the NOTES body of the Global Constraints (with the audience in double quotes) when `paigasus.iamAudienceWarns` is `true`, else `""`. `templates/NOTES.txt`, three lines. New `env.sh` functions `check_notes_pin <label>` and `check_notes <label> <body|empty> [helm args...]`, counter `NOTES_ROWS` / `NOTES_ROWS_WANT=6`. Task 4 relies on the first body line as the kind marker.

Design note (a deviation from spec § 5.2, stronger than the spec): the spec's probe ConfigMap includes `paigasus.iamAudienceNotes` directly. This plan's probe wraps the BYTES of `NOTES.txt` in a named template in the chart copy and renders that. So the probe tests what `NOTES.txt` really prints (including its whitespace trim), and N0 still pins the bytes. MEASURED: mutation M6 (NOTES.txt without the include) reds N0, N1, N2 and N5, not only N0.

- [ ] **Step 1: Write the failing rows in env.sh (header)**

Use the Edit tool on `charts/paigasus/tests/env.sh`.

old_string:
```text
#   W14 no-restart  The acknowledgement changes no pod template, so it restarts no pod.
# A second row counter reds the script when a W row call line is deleted.
set -euo pipefail
```

new_string:
```text
#   W14 no-restart  The acknowledgement changes no pod template, so it restarts no pod.
# A second row counter reds the script when a W row call line is deleted.
#
# The script also checks the SMA-691 NOTES text. No offline helm command prints NOTES, so a copy
# of the chart renders the bytes of NOTES.txt through a probe ConfigMap (check_notes_pin,
# check_notes):
#   N0 pin          The bytes of templates/NOTES.txt equal the three lines of the spec.
#   N1 default      The NOTES body, with the audience "paigasus-console".
#   N2 explicit-equal
#                   oidc.audience is set to the client id. The same body.
#   N3 distinct     oidc.audience differs from the client id. Empty.
#   N4 acknowledged The acknowledgement equals the client id. Empty.
#   N5 stale-ack    The acknowledgement names another client id. The body.
# A third row counter reds the script when an N row call line is deleted.
set -euo pipefail
```

- [ ] **Step 2: Write the failing rows in env.sh (functions and rows)**

Use the Edit tool on `charts/paigasus/tests/env.sh`.

old_string:
```text
if [ "$WARNING_ROWS" -lt "$WARNING_ROWS_WANT" ]; then
  echo "FAIL [warning rows]: $WARNING_ROWS warning row(s) ran, want $WARNING_ROWS_WANT"; ec=1
fi

if [ "$ec" -eq 0 ]; then echo "== chart env OK =="; fi
```

new_string:
```bash
if [ "$WARNING_ROWS" -lt "$WARNING_ROWS_WANT" ]; then
  echo "FAIL [warning rows]: $WARNING_ROWS warning row(s) ran, want $WARNING_ROWS_WANT"; ec=1
fi

# The NOTES text (SMA-691). No offline helm command prints NOTES: `helm install --dry-run` needs
# a cluster. So a copy of the chart gets a probe. The probe wraps the bytes of NOTES.txt in a
# named template and renders it into a ConfigMap. N0 pins those bytes.
NOTES_ROWS=0
NOTES_ROWS_WANT=6
NOTES_CHART="$TMP/notes-chart"

# check_notes_pin <label>
check_notes_pin() {
  local label="$1" out
  NOTES_ROWS=$((NOTES_ROWS + 1))
  if ! out="$(python3 -c '
import sys
want = ("{{- /* SPDX-License-Identifier: Apache-2.0 */ -}}\n"
        "{{- include \"paigasus.validate\" . -}}\n"
        "{{- include \"paigasus.iamAudienceNotes\" . -}}\n")
with open(sys.argv[1]) as fh:
    got = fh.read()
print("OK" if got == want else "templates/NOTES.txt is " + repr(got) + ", want " + repr(want))' "$CHART/templates/NOTES.txt" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

# check_notes <label> <body|empty> [helm args...]
check_notes() {
  local label="$1" want="$2"; shift 2
  local out
  NOTES_ROWS=$((NOTES_ROWS + 1))
  if ! helm template paigasus "$NOTES_CHART" "${BASE[@]+"${BASE[@]}"}" "$@" \
      --show-only templates/zz-notes-probe.yaml >"$TMP/notes.yaml" 2>"$TMP/notes.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/notes.err"; ec=1; return 0
  fi
  if ! out="$(WANT="$want" python3 -c '
import os, sys, yaml
body = "\n".join([
    "WARNING (SMA-691): the IAM audience equals oidc.clientId, so an ID token passes IAM" + chr(39) + "s audience check.",
    "IAM accepts the audience \"paigasus-console\". An OIDC ID token has the client id as its audience.",
    "IAM refuses a Keycloak ID token by its typ claim (SMA-686). Dex does not set that claim.",
    "Other IdPs are not measured.",
    "Recommended: give the API its own audience and set oidc.audience to it.",
    "Follow the order in docs/ops/RUNBOOK-chart.md section 6, or every session breaks.",
    "If your IdP cannot do this (Dex), set oidc.acknowledgeClientIdAudience to the value of",
    "oidc.clientId to remove this warning.",
])
want = body if os.environ["WANT"] == "body" else ""
with open(sys.argv[1]) as fh:
    docs = [d for d in yaml.safe_load_all(fh) if d]
cms = [d for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "notes-probe"]
if len(cms) != 1:
    print(str(len(cms)) + " notes-probe ConfigMap(s), want 1")
else:
    got = (cms[0].get("data") or {}).get("notes")
    print("OK" if got == want else "NOTES is " + repr(got) + ", want " + repr(want))' "$TMP/notes.yaml" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

if cp -R "$CHART" "$NOTES_CHART" \
  && { printf '%s\n' '{{- define "paigasus.notesProbe" -}}' && cat "$CHART/templates/NOTES.txt" \
    && printf '%s\n' '{{- end -}}'; } >"$NOTES_CHART/templates/_zz-notes-probe.tpl" \
  && printf '%s\n' 'apiVersion: v1' 'kind: ConfigMap' 'metadata:' '  name: notes-probe' 'data:' \
    '  notes: {{ include "paigasus.notesProbe" . | quote }}' >"$NOTES_CHART/templates/zz-notes-probe.yaml"; then
  :
else
  echo "FAIL [notes probe]: cannot build the probe chart under $NOTES_CHART"; ec=1
fi

check_notes_pin "N0 pin"
check_notes "N1 default"        body
check_notes "N2 explicit-equal" body  --set oidc.audience=paigasus-console
check_notes "N3 distinct"       empty --set oidc.audience=api://paigasus
check_notes "N4 acknowledged"   empty --set oidc.acknowledgeClientIdAudience=paigasus-console
check_notes "N5 stale-ack"      body  --set oidc.acknowledgeClientIdAudience=old-client

if [ "$NOTES_ROWS" -lt "$NOTES_ROWS_WANT" ]; then
  echo "FAIL [notes rows]: $NOTES_ROWS notes row(s) ran, want $NOTES_ROWS_WANT"; ec=1
fi

if [ "$ec" -eq 0 ]; then echo "== chart env OK =="; fi
```

The `&&` inside the `{ … }` group is load-bearing: with `;`, a failed `cat` is hidden because the group returns the status of its last command.

- [ ] **Step 3: Run the rows and expect them to fail**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash charts/paigasus/tests/env.sh --set ingress.host=console.example.test; echo "env rc=$?"
```

Expected (MEASURED on a scratch copy): `env rc=1`; `cat: …/templates/NOTES.txt: No such file or directory`; `FAIL [notes probe]: cannot build the probe chart under …/notes-chart`; `FAIL [N0 pin]: the checker failed` with a Python `FileNotFoundError`; N1-N5 each `FAIL […]: render failed` with `Error: parse error at (paigasus/templates/_zz-notes-probe.tpl:2): unexpected EOF`. All A and W rows pass.

- [ ] **Step 4: Append `paigasus.iamAudienceNotes` to the helper file**

Use the Edit tool on `charts/paigasus/templates/_audience.tpl`.

old_string:
```text
{{- if and (eq (include "paigasus.iamAudience" .) $clientId) (ne (toString .Values.oidc.acknowledgeClientIdAudience) $clientId) -}}
true
{{- end -}}
{{- end -}}
```

new_string:
```gotemplate
{{- if and (eq (include "paigasus.iamAudience" .) $clientId) (ne (toString .Values.oidc.acknowledgeClientIdAudience) $clientId) -}}
true
{{- end -}}
{{- end -}}

{{/*
paigasus.iamAudienceNotes: the NOTES.txt body when paigasus.iamAudienceWarns is "true", else "".
NOTES.txt only includes this helper, so charts/paigasus/tests/env.sh can test the text offline
(rows N0-N5). The first line is the marker that ci/kind/run.sh asserts. Keep the text equal to
§ 4.3 of docs/superpowers/specs/2026-09-26-sma-691-default-audience-warning-design.md.
*/}}
{{- define "paigasus.iamAudienceNotes" -}}
{{- if include "paigasus.iamAudienceWarns" . -}}
WARNING (SMA-691): the IAM audience equals oidc.clientId, so an ID token passes IAM's audience check.
IAM accepts the audience {{ include "paigasus.iamAudience" . | quote }}. An OIDC ID token has the client id as its audience.
IAM refuses a Keycloak ID token by its typ claim (SMA-686). Dex does not set that claim.
Other IdPs are not measured.
Recommended: give the API its own audience and set oidc.audience to it.
Follow the order in docs/ops/RUNBOOK-chart.md section 6, or every session breaks.
If your IdP cannot do this (Dex), set oidc.acknowledgeClientIdAudience to the value of
oidc.clientId to remove this warning.
{{- end -}}
{{- end -}}
```

- [ ] **Step 5: Create `templates/NOTES.txt`**

Create `charts/paigasus/templates/NOTES.txt` with the Write tool. The content is exactly these three lines, and the file ends with one newline after the third line:

```gotemplate
{{- /* SPDX-License-Identifier: Apache-2.0 */ -}}
{{- include "paigasus.validate" . -}}
{{- include "paigasus.iamAudienceNotes" . -}}
```

Every tag trims on both sides, so an install with no warning renders an EMPTY string and Helm prints no `NOTES:` block (spec § 4.3). `paigasus.validate` runs first, as in every template (`_helpers.tpl:50-54`).

- [ ] **Step 6: Run env.sh (both bashes), render.sh and ca-bundle.sh, and expect them to pass**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash charts/paigasus/tests/env.sh --set ingress.host=console.example.test; echo "bash3 rc=$?"
/opt/homebrew/bin/bash charts/paigasus/tests/env.sh --set ingress.host=console.example.test; echo "bash5 rc=$?"
/bin/bash charts/paigasus/tests/render.sh; echo "render rc=$?"
git diff --stat charts/paigasus/tests/golden
```

Expected: every row `ok` through `N5 stale-ack`, `== chart env OK ==`, `bash3 rc=0`, `bash5 rc=0`. render.sh (which also runs `helm lint`) prints `ok [iam-only]`, `ok [iam-and-gateway]`, `== chart render OK ==`, `render rc=0`: `helm template` executes `NOTES.txt` but does not print it, so the goldens do not change. `git diff --stat charts/paigasus/tests/golden` prints nothing. If the Homebrew bash run hangs at about 0% CPU, stop it: that is the host pipe condition (SMA-612). Record it and rely on the `/bin/bash` result.

- [ ] **Step 7: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
git add charts/paigasus/templates/_audience.tpl charts/paigasus/templates/NOTES.txt charts/paigasus/tests/env.sh
git status --short
git commit -m "$(cat <<'EOF'
feat(repo): warn in the chart NOTES when the IAM audience equals the client id (SMA-691)

helm install and helm upgrade now print a warning when an ID token passes IAM's
audience check. NOTES.txt only includes the named helper paigasus.iamAudienceNotes,
which calls the same condition as the annotation. With no warning, NOTES renders
empty and Helm prints no NOTES block. No offline helm command prints NOTES, so
env.sh renders the bytes of NOTES.txt through a probe chart (rows N0-N5).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
git log --oneline -1
```

---

### Task 4: The NOTES marker check in the kind job

**Files:**
- Modify: `ci/kind/run.sh` (header lines 11 and 17; insert before `install_a() {` at line 371; `install_a` line 382; `upgrade_b` line 474)
- Modify: `ci/kind/README.md` (Modes table lines 15 and 20; a new section before `## Reading the evidence`)
- Test: an offline harness in your session scratchpad (NOT committed). The kind job itself runs only in CI (`.github/workflows/chart.yml`).

**Interfaces:**
- Consumes: the marker line `WARNING (SMA-691): the IAM audience equals oidc.clientId` (Task 3). `h` (helm with the kind context), `die_infra` (rc 2), `die_assert` (rc 1), `RELEASE`, `NS` from `ci/kind/run.sh`.
- Produces: `NOTES_MARKER` and `assert_notes_marker <step name>` in `ci/kind/run.sh`, called as `  assert_notes_marker "install a"` and `  assert_notes_marker "upgrade b"`.

- [ ] **Step 1: Write the offline harness (the failing test)**

Write this file to `<your session scratchpad>/kind-notes-harness.sh` with the Write tool. It is a test aid only. Do not commit it.

```bash
#!/bin/bash
# Offline harness for ci/kind/run.sh assert_notes_marker (SMA-691). Not committed.
# Usage: <bash> kind-notes-harness.sh <path to ci/kind/run.sh>
set -u
SRC="$1"
FN="$(mktemp "${TMPDIR:-/tmp}/kindfn.XXXXXX")" || exit 2
sed -n '/^NOTES_MARKER=/p; /^assert_notes_marker() {/,/^}/p' "$SRC" >"$FN" || exit 2
fails=0
row() {  # $1 label, $2 stub mode, $3 wanted rc
  local rc=0
  ( set -euo pipefail
    RELEASE=paigasus; NS=paigasus
    die_infra() { printf 'infra: %s\n' "$*" >&2; exit 2; }
    die_assert() { printf 'assert: %s\n' "$*" >&2; exit 1; }
    h() {
      case "$MODE" in
        marker) printf 'NOTES:\nWARNING (SMA-691): the IAM audience equals oidc.clientId, so an ID token passes IAM%ss audience check.\nIAM accepts the audience "paigasus-console".\n' "'" ;;
        other) printf 'NOTES:\nWARNING (SMA-691): the IAM audience is oidc.clientId\n' ;;
        empty) printf '' ;;
        fail) echo 'Error: release: not found' >&2; return 1 ;;
      esac
    }
    MODE="$2"
    . "$FN"
    assert_notes_marker "install a"
  ) >/dev/null 2>&1 || rc=$?
  if [ "$rc" = "$3" ]; then echo "  ok [$1]: rc=$rc"; else echo "FAIL [$1]: rc=$rc, want $3"; fails=$((fails + 1)); fi
}
n="$(grep -c . "$FN")"
[ "$n" -ge 5 ] || { echo "FAIL [extract]: $n line(s) extracted from $SRC"; rm -f "$FN"; exit 1; }
row "marker present"       marker 0
row "marker text changed"  other  1
row "notes empty"          empty  1
row "helm get notes fails" fail   2
for site in 'install a' 'upgrade b'; do
  c="$(grep -c -x "  assert_notes_marker \"$site\"" "$SRC")"
  if [ "$c" = 1 ]; then echo "  ok [call site $site]"; else echo "FAIL [call site $site]: $c line(s), want 1"; fails=$((fails + 1)); fi
done
rm -f "$FN"
[ "$fails" = 0 ] && echo "== kind notes harness OK ==" || exit 1
```

- [ ] **Step 2: Run the harness and expect it to fail**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
/bin/bash <your session scratchpad>/kind-notes-harness.sh ci/kind/run.sh; echo "rc=$?"
```

Expected (MEASURED): `FAIL [extract]: 0 line(s) extracted from ci/kind/run.sh`, `rc=1`.

- [ ] **Step 3: Add the marker and the function**

Use the Edit tool on `ci/kind/run.sh`.

old_string:
```text
# --------------------------------------------------------------------------- install/upgrade

install_a() {
```

new_string:
```bash
# --------------------------------------------------------------------------- install/upgrade

# The first line of the chart's NOTES when the IAM audience equals oidc.clientId (SMA-691,
# charts/paigasus/templates/_audience.tpl). values/a.yaml and b.yaml keep the default audience,
# so this job is the end-to-end positive control of NOTES.txt, on install and on upgrade.
NOTES_MARKER='WARNING (SMA-691): the IAM audience equals oidc.clientId'
# Capture first, then match with `case`: no pipe into an early-exit reader (actionlint check 13).
assert_notes_marker() {  # $1 = the step name, for the messages
  local notes
  notes="$(h get notes "$RELEASE" --namespace "$NS")" || die_infra "$1: helm get notes $RELEASE failed"
  case "$notes" in
    *"$NOTES_MARKER"*) echo "  $1: NOTES shows the SMA-691 audience warning" ;;
    *) die_assert "$1: the release NOTES have no line '$NOTES_MARKER'. The default audience must show the SMA-691 warning" ;;
  esac
}

install_a() {
```

The marker is inside double quotes in the `case` pattern, so its `(` and `)` match literally (MEASURED under `/bin/bash` 3.2.57 and Homebrew bash 5.3.15).

- [ ] **Step 4: Call it at the end of `install_a` and `upgrade_b`**

Use the Edit tool on `ci/kind/run.sh`.

old_string:
```text
  echo "  phase A: $gw exists"
  echo "== install a: done =="
```

new_string:
```text
  echo "  phase A: $gw exists"
  assert_notes_marker "install a"
  echo "== install a: done =="
```

Use the Edit tool on `ci/kind/run.sh`.

old_string:
```text
  settle_gateway_404
  echo "== upgrade b: done =="
```

new_string:
```text
  settle_gateway_404
  assert_notes_marker "upgrade b"
  echo "== upgrade b: done =="
```

- [ ] **Step 5: Update the mode lines in the run.sh header**

Use the Edit tool on `ci/kind/run.sh`.

old_string:
```text
#   run.sh install a       helm install with values/a.yaml (both zones)
```

new_string:
```text
#   run.sh install a       helm install with values/a.yaml (both zones), then the NOTES check
```

Use the Edit tool on `ci/kind/run.sh`.

old_string:
```text
#   run.sh upgrade b       helm upgrade with a.yaml + b.yaml (gateway zone off), then settle
```

new_string:
```text
#   run.sh upgrade b       helm upgrade with a.yaml + b.yaml (gateway zone off), then settle and
#                          the NOTES check
```

- [ ] **Step 6: Run the harness and the syntax checks, and expect them to pass**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
/bin/bash <your session scratchpad>/kind-notes-harness.sh ci/kind/run.sh; echo "bash3 rc=$?"
/opt/homebrew/bin/bash <your session scratchpad>/kind-notes-harness.sh ci/kind/run.sh; echo "bash5 rc=$?"
/bin/bash -n ci/kind/run.sh; echo "syntax3 rc=$?"
/opt/homebrew/bin/bash -n ci/kind/run.sh; echo "syntax5 rc=$?"
```

Expected: under each bash, `ok [marker present]: rc=0`, `ok [marker text changed]: rc=1`, `ok [notes empty]: rc=1`, `ok [helm get notes fails]: rc=2`, `ok [call site install a]`, `ok [call site upgrade b]`, `== kind notes harness OK ==`, rc 0. Both `syntax` lines rc 0. (The four function rows are MEASURED on a scratch copy of the function under both bashes.)

- [ ] **Step 7: Update `ci/kind/README.md`**

Use the Edit tool on `ci/kind/README.md`.

old_string:
```text
| `bash ci/kind/run.sh install a` | `helm install` with `values/a.yaml` (both zones, the CA bundle set) |
```

new_string:
```text
| `bash ci/kind/run.sh install a` | `helm install` with `values/a.yaml` (both zones, the CA bundle set), then the NOTES check (SMA-691) |
```

Use the Edit tool on `ci/kind/README.md`.

old_string:
```text
| `bash ci/kind/run.sh upgrade b` | `helm upgrade` with `a.yaml` + `b.yaml` (the gateway zone off), then the settle step |
```

new_string:
```text
| `bash ci/kind/run.sh upgrade b` | `helm upgrade` with `a.yaml` + `b.yaml` (the gateway zone off), then the settle step and the NOTES check (SMA-691) |
```

Use the Edit tool on `ci/kind/README.md`.

old_string:
```text
## Reading the evidence
```

new_string:
```text
## The NOTES check (SMA-691)

`install a` and `upgrade b` end with `helm get notes`. The release NOTES must contain the line `WARNING (SMA-691): the IAM audience equals oidc.clientId`. `values/a.yaml` and `values/b.yaml` keep the default audience, so the chart must show this warning. This check is the end-to-end positive control of `charts/paigasus/templates/NOTES.txt`, on install and on upgrade.

- A missing line is rc 1. A failed `helm get notes` is rc 2.
- The check is inside `install a`. A NOTES failure there stops the later steps of that run. The rows N0-N5 in `charts/paigasus/tests/env.sh` test the NOTES text offline, on every pull request, so they find a text change first.
- This job is not a required check. If a change deletes the NOTES check, nothing fails (SMA-691 spec, residual R2).

## Reading the evidence
```

- [ ] **Step 8: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
git add ci/kind/run.sh ci/kind/README.md
git status --short
git commit -m "$(cat <<'EOF'
feat(ci): assert the chart's audience warning in the kind job NOTES (SMA-691)

install a and upgrade b now read helm get notes and require the SMA-691 marker
line. The kind values keep the default audience, so this is the end-to-end
positive control of NOTES.txt on install and on upgrade. The output is captured
first and matched with case, with no pipe into an early-exit reader. A missing
line is rc 1; a failed helm get notes is rc 2.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
git log --oneline -1
```

---

### Task 5: The documents

All text in this task is ASD-STE100 Simplified Technical English. Copy it exactly.

**Files:**
- Modify: `charts/paigasus/values.yaml:81-87` (the `oidc.audience` comment)
- Modify: `docs/ops/RUNBOOK-chart.md` (§ 1 table lines 25-26; § 5 table after line 85; § 6 item 1 lines 97-109; § 6 lines 131-158)
- Modify: `charts/paigasus/README.md` (lines 115-135, "The access-token audience")

**Interfaces:**
- Consumes: the names and behavior of Tasks 1-4 (value, helpers, annotation key, marker, rows W1-W14 and N0-N5).
- Produces: documents only.

- [ ] **Step 1: Check that the documents do not yet say the new facts (the failing check)**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
grep -c 'acknowledgeClientIdAudience' docs/ops/RUNBOOK-chart.md charts/paigasus/README.md
grep -c 'Migration order' docs/ops/RUNBOOK-chart.md
grep -c 'Set this value only when your IdP writes a different value' charts/paigasus/values.yaml
```

Expected: `docs/ops/RUNBOOK-chart.md:0`, `charts/paigasus/README.md:0`, then `0`, then `1`.

- [ ] **Step 2: Rewrite the `oidc.audience` comment in values.yaml**

Use the Edit tool on `charts/paigasus/values.yaml`.

old_string:
```text
  audience: ""           # NOT required. This is the access-token audience that IAM accepts.
                         # Empty: IAM uses oidc.clientId as the audience.
                         # Set this value only when your IdP writes a different value into the
                         # access token's `aud` claim. Example: Okta uses api://default.
                         # A set value REPLACES oidc.clientId. IAM then does not accept
                         # oidc.clientId as the audience.
                         # Quote this value in a values file. See docs/ops/RUNBOOK-chart.md § 6.
```

new_string:
```text
  audience: ""           # NOT required, but recommended. This is the access-token audience that
                         # IAM accepts. Empty: IAM uses oidc.clientId as the audience. An OIDC ID
                         # token has the client id as its audience, so an ID token then passes
                         # IAM's audience check, and the chart shows a warning (SMA-691).
                         # Recommended: give the API its own audience in the IdP, in the access
                         # token only, and set it here. Example: api://paigasus.
                         # A set value REPLACES oidc.clientId. IAM then does not accept
                         # oidc.clientId as the audience. Follow the migration order in
                         # docs/ops/RUNBOOK-chart.md § 6, or IAM refuses every live session.
                         # Quote this value in a values file.
```

- [ ] **Step 3: Update the runbook values table (§ 1)**

Use the Edit tool on `docs/ops/RUNBOOK-chart.md`.

old_string:
```text
| `oidc.clientId` | yes | The console's OIDC client. IAM also uses it as the access-token audience. Set `oidc.audience` to use a different value (§ 6) |
| `oidc.audience` | no | The access-token audience IAM accepts. Default: `oidc.clientId`. Set it only when the IdP puts another value in `aud` (§ 6) |
```

new_string:
```text
| `oidc.clientId` | yes | The console's OIDC client. By default IAM also uses it as the access-token audience. Then an ID token passes IAM's audience check, and the chart shows a warning (§ 6) |
| `oidc.audience` | no | The access-token audience IAM accepts. Default: `oidc.clientId`. Recommended: a dedicated API audience. Follow the migration order in § 6 |
| `oidc.acknowledgeClientIdAudience` | no | Set it to the value of `oidc.clientId` to remove the audience warning (§ 6). It does not change what IAM accepts |
```

- [ ] **Step 4: Add the restart row (§ 5)**

Use the Edit tool on `docs/ops/RUNBOOK-chart.md`.

old_string:
```text
| `oidc.audience` | the IAM pod, not the consoles | it changes `IAM_AUTHN__ISSUERS` in the IAM pod template. IAM has one replica and `maxSurge: 0` (`templates/backend-deployment.yaml`). IAM is not available during the restart. |
```

new_string:
```text
| `oidc.audience` | the IAM pod, not the consoles | it changes `IAM_AUTHN__ISSUERS` in the IAM pod template. IAM has one replica and `maxSurge: 0` (`templates/backend-deployment.yaml`). IAM is not available during the restart. |
| `oidc.acknowledgeClientIdAudience` | nothing | it changes only the IAM Deployment's `metadata` annotation and the NOTES, not a pod template (`tests/env.sh` row W14) |
```

- [ ] **Step 5: Rewrite § 6 item 1**

Use the Edit tool on `docs/ops/RUNBOOK-chart.md`.

old_string:
```text
1. **Audience.** The access token's `aud` claim must contain the audience that IAM accepts. IAM
   accepts `oidc.audience` when it is set. It accepts `oidc.clientId` when `oidc.audience` is not
   set. Set `oidc.audience` in two cases. First, set it when you cannot make the IdP put the
   client id into `aud`. Second, set it when the IdP's ID token has no Keycloak `typ` claim. The
   paragraph after this list tells you how to choose the value in that case. The value replaces
   the client id. It does not add another value next to the client id. Before you choose the
   value, decode a real access token and read its `aud` claim. The value helps only when the IdP
```

new_string:
```text
1. **Audience.** The access token's `aud` claim must contain the audience that IAM accepts. IAM
   accepts `oidc.audience` when it is set. It accepts `oidc.clientId` when `oidc.audience` is not
   set. The recommended setup is a dedicated API audience. See "The recommended audience setup"
   after this list. The value replaces the client id. It does not add another value next to the
   client id. Before you choose the value, decode a real access token and read its `aud` claim.
   The value helps only when the IdP
```

Then run `sed -n 95,112p docs/ops/RUNBOOK-chart.md` and read it. The next line must continue with `issues a JWT access token for the console's scopes`. The short line "The value helps only when the IdP" is intended; Markdown joins the lines.

- [ ] **Step 6: Replace the Dex paragraph and the Keycloak example in § 6**

Use the Edit tool on `docs/ops/RUNBOOK-chart.md`. The old_string runs from `The check does not protect an IdP` to `is a complete example.` (current lines 131-158).

old_string:
~~~~text
The check does not protect an IdP whose ID token has no `typ` claim. Dex is an example. Decode a
real ID token and a real access token from your IdP. If the ID token has no `typ: ID`, find an
audience for `oidc.audience`. The access token's `aud` must contain it. The ID token's `aud` must
not contain it. If your IdP cannot do this, IAM accepts its ID token as a bearer token. For Dex,
both tokens have the same `aud`, so this remedy does not work.

The console requests the scopes `openid profile email offline_access`.

**Keycloak example.** Keycloak does not put the client id into the access token's `aud` by
default. Add an audience mapper to the client:

```json
{
  "name": "paigasus-console-audience",
  "protocol": "openid-connect",
  "protocolMapper": "oidc-audience-mapper",
  "config": {
    "included.custom.audience": "paigasus-console",
    "id.token.claim": "false",
    "access.token.claim": "true"
  }
}
```

Put `basic`, `profile`, `email` and `offline_access` in the client's default client scopes. In
Keycloak 25 and later the `sub` claim comes from the `basic` scope. Give each user an email
address and the `offline_access` role. The kind job's realm, `ci/kind/realm/paigasus-realm.json`,
is a complete example.
~~~~

new_string:
~~~~text
The check does not protect an IdP whose ID token has no `typ` claim. Dex is an example. For such
an IdP, the audience setup below is the only protection.

The console requests the scopes `openid profile email offline_access`.

**The recommended audience setup (SMA-691).** An OIDC ID token has the client id as its `aud`.
So when the IAM audience equals `oidc.clientId`, an ID token passes IAM's audience check. This is
the default. Give the API its own audience, for example `api://paigasus`. Put it into the access
token's `aud`. Do not put it into the ID token's `aud`. Then set `oidc.audience` to it.

**Migration order.** A set `oidc.audience` replaces the client id. IAM then refuses every live
access token whose `aud` holds only the client id. IAM also restarts with a gap, because it has
one replica and `maxSurge: 0` (§ 5). Do the steps in this order:

1. In the IdP, add the API audience to the access token, next to the client id. Do not add it to
   the ID token.
2. Wait for one access-token lifetime. Then every live access token has the new audience.
3. Set `oidc.audience` to the API audience and upgrade. IAM restarts and then accepts only the
   API audience.
4. Optional: remove the client id from the access token's `aud`.

If you do step 3 before step 1, IAM refuses the token of every console session. An IdP change
that replaces `aud`, and does not add to it, has the same result.

**The warning.** The chart shows a warning when both of these conditions are true:

- The IAM audience equals `oidc.clientId`. The IAM audience is `oidc.audience`, or
  `oidc.clientId` when `oidc.audience` is empty.
- `oidc.acknowledgeClientIdAudience` does not equal `oidc.clientId`.

The warning has two forms:

- `helm install` and `helm upgrade` print it in the release NOTES. Flux's helm-controller also
  stores the NOTES in the release. The first line is `WARNING (SMA-691): the IAM audience equals
  oidc.clientId, so an ID token passes IAM's audience check.`
- The IAM backend Deployment gets the annotation `paigasus.io/iam-audience-warning` in its
  `metadata`. A tool that renders with `helm template`, for example Argo CD, does not show NOTES.
  Read the annotation there. The annotation is not on the pod template, so it does not restart a
  pod.

The warning does not stop an install or an upgrade. It does not change what IAM accepts. A GitOps
sync does not fail because of it.

**The acknowledgement.** If your IdP cannot give the API its own audience, set
`oidc.acknowledgeClientIdAudience` to the value of `oidc.clientId`. The warning then does not
show. The acknowledgement does not change what IAM accepts. IAM still accepts an ID token as a
bearer token, except a Keycloak ID token (SMA-686). The value must equal the client id exactly,
with the same letter case. `true` does not work. When you change `oidc.clientId`, the warning
shows again. Quote the value in a values file. An unquoted large number can change to an
exponent form (`1e+06`), and the warning then continues to show.

**No warning does not mean a safe setup.** Any `oidc.audience` that differs from the client id
removes the warning. If the IdP also puts that audience into the ID token, the ID token still
passes IAM's audience check, and nothing warns. Decode a real ID token. Its `aud` must not
contain the value of `oidc.audience`.

**Per IdP. Not measured.** These lines state what each IdP offers. This chart did not measure
them.

- **Keycloak.** Add an "Audience" protocol mapper to the console client, or to a client scope of
  that client. Set "Included Custom Audience" to the API audience. Set "Add to access token" on
  and "Add to ID token" off. The mapper adds a value to `aud`. It does not replace `aud`. Keycloak
  example 2 below shows the mapper. IAM also refuses a Keycloak ID token by its `typ` claim
  (SMA-686).
- **Okta.** Use a custom authorization server whose audience is the API identifier. See the Okta
  example below.
- **Auth0.** The console cannot send the `audience` parameter today. It sends only `scope`
  (`ts/packages/paigasus-auth/src/adapters/oidc.ts`). The tenant "Default Audience" setting is a
  possible path. It is not measured.
- **Entra ID.** An access token for an API application ID URI needs a scope of that API. The
  console reads its scopes from `PAIGASUS_OIDC_SCOPES`, but the chart has no value for it. So
  Entra ID cannot use a dedicated audience with this chart today. A follow-up issue tracks a
  configurable scope list.
- **Dex.** Dex gives the ID token and the access token the same `aud`. No audience setting helps.
  Set `oidc.acknowledgeClientIdAudience` to remove the warning. IAM still accepts a Dex ID token
  as a bearer token. SMA-686 residual R1 stays open for Dex.

**Keycloak example 1: the kind job's setup. This setup shows the warning.** Keycloak does not put
the client id into the access token's `aud` by default. The kind job adds an audience mapper to
the client. The mapper adds the client id, so the IAM audience equals `oidc.clientId`:

```json
{
  "name": "paigasus-console-audience",
  "protocol": "openid-connect",
  "protocolMapper": "oidc-audience-mapper",
  "config": {
    "included.custom.audience": "paigasus-console",
    "id.token.claim": "false",
    "access.token.claim": "true"
  }
}
```

Put `basic`, `profile`, `email` and `offline_access` in the client's default client scopes. In
Keycloak 25 and later the `sub` claim comes from the `basic` scope. Give each user an email
address and the `offline_access` role. The kind job's realm, `ci/kind/realm/paigasus-realm.json`,
is a complete example of this setup.

**Keycloak example 2: a dedicated API audience. Not tested in the kind job.** Add this mapper to
the console client (step 1 of the migration order). Keep the mapper of example 1 until step 4:

```json
{
  "name": "paigasus-api-audience",
  "protocol": "openid-connect",
  "protocolMapper": "oidc-audience-mapper",
  "config": {
    "included.custom.audience": "api://paigasus",
    "id.token.claim": "false",
    "access.token.claim": "true"
  }
}
```

Then set `oidc.audience=api://paigasus` (step 3). Quote the value in a values file.
~~~~

- [ ] **Step 7: Update the chart README**

Use the Edit tool on `charts/paigasus/README.md`.

old_string:
```text
- A change of the value restarts the IAM pod and no console pod.

`tests/env.sh` holds the rows: `A1 unset`, `A2 reuse-values-no-key` (`--set oidc.audience=null`),
`A3 set`, `A4 number`, `A5 number-in-file` and `A6 restart-scope`. A row counter reds the script
when a row call line is deleted. See `docs/ops/RUNBOOK-chart.md` § 6 for when to set the value.
```

new_string:
```text
- A change of the value restarts the IAM pod and no console pod.
- **The warning (SMA-691).** When the IAM audience equals `oidc.clientId`, an ID token passes
  IAM's audience check. The chart then shows a warning in two places: the release NOTES
  (`templates/NOTES.txt`) and the annotation `paigasus.io/iam-audience-warning` in the IAM backend
  Deployment's `metadata`. `oidc.acknowledgeClientIdAudience`, set to the value of
  `oidc.clientId`, removes both. It does not change what IAM accepts. The helpers are in
  `templates/_audience.tpl`. Both signals call `paigasus.iamAudienceWarns`. The recommended setup
  is a dedicated API audience in `oidc.audience`.
- **NOTES has no offline render.** `helm template` executes `NOTES.txt` but does not print it, and
  `helm install --dry-run` needs a cluster. So `tests/env.sh` wraps the bytes of `NOTES.txt` in a
  named template in a copy of the chart and renders it through a probe ConfigMap. The kind job
  checks the NOTES of the real release (`ci/kind/README.md`).

`tests/env.sh` holds the rows: `A1 unset`, `A2 reuse-values-no-key` (`--set oidc.audience=null`),
`A3 set`, `A4 number`, `A5 number-in-file` and `A6 restart-scope`. For the warning annotation it
holds `W1 default` to `W14 no-restart`. For the NOTES text it holds `N0 pin` (the bytes of
`NOTES.txt`) and `N1 default` to `N5 stale-ack`. Three row counters red the script when a row call
line is deleted. See `docs/ops/RUNBOOK-chart.md` § 6 for the recommended setup and the migration
order.
```

- [ ] **Step 8: Check the documents**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
grep -c 'acknowledgeClientIdAudience' docs/ops/RUNBOOK-chart.md charts/paigasus/README.md
grep -c 'Migration order' docs/ops/RUNBOOK-chart.md
grep -c 'Set this value only when your IdP writes a different value' charts/paigasus/values.yaml
grep -n 'section 6\|§ 6' charts/paigasus/templates/_audience.tpl charts/paigasus/templates/backend-deployment.yaml
/bin/bash charts/paigasus/tests/env.sh --set ingress.host=console.example.test; echo "env rc=$?"
/bin/bash charts/paigasus/tests/render.sh; echo "render rc=$?"
```

Expected: the runbook count is at least 5 and the README count at least 1; `Migration order` is `1`; the old values sentence is `0`; the grep shows exactly two lines, both with ASCII `section 6` (the annotation value in `backend-deployment.yaml` and the NOTES body line in `_audience.tpl`), and no `§ 6`; `env rc=0`; `render rc=0` (the values.yaml comments do not render).

Read the rendered runbook § 6 once from top to bottom (`sed -n 90,260p docs/ops/RUNBOOK-chart.md`). Check that every sentence is short, active and without idiom, and that no paragraph has more than six sentences.

- [ ] **Step 9: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
git add charts/paigasus/values.yaml docs/ops/RUNBOOK-chart.md charts/paigasus/README.md
git status --short
git commit -m "$(cat <<'EOF'
docs(repo): recommend a dedicated IAM audience in the chart runbook (SMA-691)

The runbook, the chart README and the values comment now recommend a dedicated
API audience in oidc.audience. Section 6 gives the migration order, the exact
warning condition, the acknowledgement and its limits, a line per IdP (not
measured), and the fact that no warning does not mean a safe setup. The first
Keycloak example is now marked as the kind job's setup, which shows the warning.
A second example uses a dedicated API audience.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
git log --oneline -1
```

---

### Task 6: The mutation battery (spec § 5.6) and the measurements file

**Files:**
- Create: `docs/superpowers/specs/2026-09-26-sma-691-measurements.md`
- Mutated and restored (no net change): `charts/paigasus/templates/_audience.tpl`, `charts/paigasus/templates/backend-deployment.yaml`, `charts/paigasus/templates/NOTES.txt`, `charts/paigasus/tests/env.sh`, `ci/kind/run.sh`

**Interfaces:**
- Consumes: everything from Tasks 1-4. The kind harness from Task 4 Step 1.
- Produces: the measurements file only.

- [ ] **Step 1: Run the mutations one at a time**

For each row: apply the mutation with the Edit tool (old_string and new_string exactly as shown), run the command, compare the FAIL lines with the "Expected red rows" column, then remove the mutation with the inverse Edit. Then run the command once more and confirm `== chart env OK ==` before the next row. Record the exact FAIL labels you see.

The command for each mutation:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash charts/paigasus/tests/env.sh --set ingress.host=console.example.test; echo "env rc=$?"
```

For M4, M5 and M10 also run `/bin/bash charts/paigasus/tests/render.sh; echo "render rc=$?"`. For M15 run the kind harness instead: `/bin/bash <your session scratchpad>/kind-notes-harness.sh ci/kind/run.sh; echo "rc=$?"`.

The "Expected red rows" column is MEASURED on a scratch copy of the chart while this plan was written. The `\|` in the table is a Markdown escape for `|`.

| # | File | old_string (exact) | new_string (exact) | Spec § 5.6 says | Expected red rows |
|---|---|---|---|---|---|
| M1 | `templates/_audience.tpl` | `{{- if and (eq (include "paigasus.iamAudience" .) $clientId) (ne (toString .Values.oidc.acknowledgeClientIdAudience) $clientId) -}}` | `{{- if ne (toString .Values.oidc.acknowledgeClientIdAudience) $clientId -}}` | W4, N3 | W4, N3 |
| M2 | `templates/_audience.tpl` | `{{- if and (eq (include "paigasus.iamAudience" .) $clientId) (ne (toString .Values.oidc.acknowledgeClientIdAudience) $clientId) -}}` | `{{- if eq (include "paigasus.iamAudience" .) $clientId -}}` | W5, N4 | W5, N4 |
| M3 | `templates/_audience.tpl` | `(ne (toString .Values.oidc.acknowledgeClientIdAudience) $clientId)` | `(ne (toString .Values.oidc.acknowledgeClientIdAudience) "true")` | W5, W6 | W5, W6, N4 |
| M4 | `templates/backend-deployment.yaml` | the whole metadata block from `{{- if and (eq $id "iam") (include "paigasus.iamAudienceWarns" $root) }}` to its `{{- end }}` (10 lines) | (empty); and after the line `        checksum/pepper-secret: {{ $z.backend.apiKeysSecretVersion \| sha256sum }}` insert the three lines `{{- if and (eq $id "iam") (include "paigasus.iamAudienceWarns" $root) }}`, `        paigasus.io/iam-audience-warning: "<the exact value>"`, `{{- end }}` | W1, W9 | W1, W2, W3, W6, W7, W8, W9, W11, W12, W13, W14; render.sh both goldens |
| M5 | `templates/backend-deployment.yaml` | `{{- if and (eq $id "iam") (include "paigasus.iamAudienceWarns" $root) }}` | `{{- if include "paigasus.iamAudienceWarns" $root }}` | W1 (count) | NONE: equivalent mutant (see Step 2); render.sh green |
| M6 | `templates/NOTES.txt` | the line `{{- include "paigasus.iamAudienceNotes" . -}}` and its newline | (empty) | N0 | N0, N1, N2, N5 |
| M7 | `templates/_audience.tpl` | `WARNING (SMA-691): the IAM audience equals oidc.clientId,` | `WARNING (SMA-691): the IAM audience is oidc.clientId,` | N1 | N1, N2, N5 |
| M8a | `tests/env.sh` | the line `check_warning "W3 explicit-equal"        present      --set oidc.audience=paigasus-console` and its newline | (empty) | the row counter | `FAIL [warning rows]: 13 warning row(s) ran, want 14` |
| M8b | `tests/env.sh` | the line `check_notes "N5 stale-ack"      body  --set oidc.acknowledgeClientIdAudience=old-client` and its newline | (empty) | the row counter | `FAIL [notes rows]: 5 notes row(s) ran, want 6` |
| M9a | `templates/_audience.tpl` | `{{- .Values.oidc.audience \| default .Values.oidc.clientId \| toString -}}` | `{{- .Values.oidc.audience \| default .Values.oidc.clientId -}}` | A4, A5 ("drop toString") | NONE: equivalent mutant (see Step 2) |
| M9b | `templates/_audience.tpl` | `{{- .Values.oidc.audience \| default .Values.oidc.clientId \| toString -}}` | `{{- .Values.oidc.audience \| toString -}}` | (replaces M9a) | A1, A2, W1, W2, W6, W7, W8, N1, N5 |
| M10 | `templates/backend-deployment.yaml` | the two lines `{{- if and (eq $id "iam") (include "paigasus.iamAudienceWarns" $root) }}` and `{{- /*` | the same first line and `{{/*` | (plan) | env.sh green; render.sh `FAIL [iam-only]` and `FAIL [iam-and-gateway]` |
| M11 | `templates/NOTES.txt` | `{{- include "paigasus.iamAudienceNotes" . -}}` | `{{ include "paigasus.iamAudienceNotes" . }}` | (plan) | N0 only (the probe's `{{- end -}}` trims the extra newline, so N0 is the only guard) |
| M12 | `templates/_audience.tpl` | `(ne (toString .Values.oidc.acknowledgeClientIdAudience) $clientId)` | `(ne (toString .Values.oidc.acknowledgeClientIdAudience \| lower) (lower $clientId))` | (Review Focus 2) | W12 |
| M13 | `templates/_audience.tpl` | `(ne (toString .Values.oidc.acknowledgeClientIdAudience) $clientId)` | `(ne .Values.oidc.acknowledgeClientIdAudience $clientId)` | (Review Focus 1) | W6, W10, W11 (`render failed`: incompatible types for comparison) |
| M14 | `templates/backend-deployment.yaml` | `checksum/pepper-secret: {{ $z.backend.apiKeysSecretVersion \| sha256sum }}` | `checksum/pepper-secret: {{ print $z.backend.apiKeysSecretVersion $root.Values.oidc.acknowledgeClientIdAudience \| sha256sum }}` | (Review Focus 4) | W14 (`iam-backend: spec.template differs; it must be equal`) |
| M15 | `ci/kind/run.sh` | the line `  assert_notes_marker "upgrade b"` and its newline | (empty) | nothing; see R2 | no committed check reds; the scratch harness reds `FAIL [call site upgrade b]: 0 line(s), want 1` |

In M4, `<the exact value>` is the annotation value from the Global Constraints, in double quotes. The inserted `if` line has no indent; the annotation line has 8 spaces.

If a mutation reds FEWER rows than the "Expected red rows" column, stop and report: a row does not bite. If it reds more, record the extra rows.

- [ ] **Step 2: Understand the two equivalent mutants before you record them**

- **M5.** `paigasus.validate` refuses `zones.gateway.backend.deploy=true` while the gateway zone is on (`_helpers.tpl:81-83`), and `backend-deployment.yaml` renders a backend only when `enabled` and `deploy` are both true. So every successful render has exactly ONE backend Deployment, the IAM one. Without `eq $id "iam"`, the render is identical. No test can red it. Keep `eq $id "iam"`: it protects against a future chart that deploys a second backend. W1's "exactly once" and W13 (gateway on, three Deployments) still catch the annotation on a CONSOLE Deployment.
- **M9a.** `include` renders the helper to TEXT. The text/template printer writes an int64 `12345` and a float64 `12345` as `12345`, the same as `toString`. So `toString` inside `paigasus.iamAudience` changes no render. The spec's row "drop `toString` on line 114 → A4, A5" was true for the inline expression before Task 1. After the refactor, M9b (drop `default`) is the mutation that proves A1 and A2 read through the helper.

- [ ] **Step 3: Confirm the tree is clean after the battery**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git status --short
/bin/bash charts/paigasus/tests/env.sh --set ingress.host=console.example.test; echo "env rc=$?"
/bin/bash charts/paigasus/tests/render.sh; echo "render rc=$?"
/bin/bash <your session scratchpad>/kind-notes-harness.sh ci/kind/run.sh; echo "harness rc=$?"
```

Expected: `git status --short` prints nothing; `env rc=0`; `render rc=0`; `harness rc=0`.

- [ ] **Step 4: Write the measurements file**

Create `docs/superpowers/specs/2026-09-26-sma-691-measurements.md` in the style of `docs/superpowers/specs/2026-09-25-sma-686-measurements.md` (a title, a "Measured on" paragraph, tables). Fill the "Measured red rows" column with the FAIL labels you saw in Step 1, not with the expected column. Use this content, and replace each `<…>` with the measured value:

~~~~markdown
# SMA-691 measurements: the audience warning and its mutation battery

Measured on <date> in the worktree of branch `feature/sma-691-chart-default-audience`, with Helm
v3.22.0+g144ca65, `/bin/bash` 3.2.57 and Python <python3 --version> with PyYAML 6.0.3. The spec
is `2026-09-26-sma-691-default-audience-warning-design.md`. Each mutation was applied with an
edit, measured, and removed with the inverse edit. No mutation was committed.

## Render facts

| Fact | Result |
|---|---|
| `helm install --dry-run=client` with no cluster | fails: `Kubernetes cluster unreachable` |
| `helm template` with a template error in `NOTES.txt` | fails (rc 1); `helm lint` also fails |
| `helm template` output | does not contain the NOTES text |
| `toString` of an absent key (`--set oidc.acknowledgeClientIdAudience=null`, or `~` in a values file) | `<nil>` |
| `toString` of `12345` from a values file (float64) | `12345` |
| `toString` of `1000000` from a values file (float64) | `1e+06` |
| Golden diff after `render.sh --update` | `2 0` per file: `  annotations:` and the annotation line |

## The mutation battery (spec § 5.6)

The command after each mutation was `/bin/bash charts/paigasus/tests/env.sh --set
ingress.host=console.example.test`. M4, M5 and M10 also ran `render.sh`. M15 ran the offline kind
harness (not committed).

| # | Mutation | Spec § 5.6 expects | Measured red rows |
|---|---|---|---|
| M1 | drop condition 1 (audience equals client id) | W4, N3 | <…> |
| M2 | drop condition 2 (the acknowledgement) | W5, N4 | <…> |
| M3 | compare the acknowledgement with `"true"` | W5, W6 | <…> |
| M4 | the annotation in `spec.template.metadata.annotations` | W1, W9 | <…> |
| M5 | drop `eq $id "iam"` | W1 (count) | <…> |
| M6 | `NOTES.txt` without the include | N0 | <…> |
| M7 | change the marker line text | N1 | <…> |
| M8a | delete the W3 call line | the row counter | <…> |
| M8b | delete the N5 call line | the row counter | <…> |
| M9a | drop `toString` in `paigasus.iamAudience` | A4, A5 | <…> |
| M9b | drop `default` in `paigasus.iamAudience` | (replaces M9a) | <…> |
| M10 | the annotation's Go comment without left trim | (plan) | <…> |
| M11 | the NOTES include without trim | (plan) | <…> |
| M12 | a case-insensitive compare | (plan) | <…> |
| M13 | the compare without `toString` | (plan) | <…> |
| M14 | the acknowledgement fed into a pod checksum | (plan) | <…> |
| M15 | delete the `upgrade b` NOTES check in `ci/kind/run.sh` | nothing (R2) | <…> |

## Two equivalent mutants

- **M5.** `paigasus.validate` refuses the gateway backend while the gateway zone is on, so every
  successful render has exactly one backend Deployment. Without `eq $id "iam"`, the render does
  not change. No row can fail. The spec expected W1 to fail. `eq $id "iam"` stays as protection
  for a future second backend.
- **M9a.** `include` renders the helper to text, and the template printer writes a number as its
  digits. So `toString` in `paigasus.iamAudience` changes no render. The spec expected A4 and A5
  to fail. That was true for the inline expression before the refactor. M9b is the mutation that
  proves A1 and A2 read through the helper.
~~~~

- [ ] **Step 5: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
git add docs/superpowers/specs/2026-09-26-sma-691-measurements.md
git status --short
git commit -m "$(cat <<'EOF'
docs(repo): record the SMA-691 mutation battery and render facts

Each mutation of spec section 5.6 was applied, measured and removed. Two are
equivalent mutants: dropping the iam check on the annotation (no valid render
has a second backend Deployment) and dropping toString inside the audience
helper (include already prints a number as its digits).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
git log --oneline -1
```

---

### Task 7: Final verification

No new code. Fix a failure in the task that owns it, with a NEW commit (never `--amend`).

**Files:** none changed, unless a check fails.

**Interfaces:** none.

- [ ] **Step 1: Run the static gate for the chart (all chart scripts included)**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
/bin/bash ci/helm-render/run.sh --self-test; echo "self-test rc=$?"
/bin/bash ci/helm-render/run.sh --negative-control; echo "negative-control rc=$?"
/bin/bash ci/helm-render/run.sh; echo "gate rc=$?"
```

`ci/helm-render/run.sh` runs under `/bin/bash` 3.2.57 and bash 5 (its README). The full mode runs checks 1-4 and 7, then every `charts/paigasus/tests/*.sh` as `"$BASH" <script> --set ingress.host=console.example.test`.

Expected: `self-test rc=0`; `negative-control rc=0` with `OK` for each of the six fixtures (they are unchanged, and `_helpers.tpl` is unchanged); `gate rc=0` with `PASS  [6 <script>]` for `ca-bundle.sh`, `env.sh`, `ingress.sh`, `maps.sh`, `names.sh`, `refusals.sh`, `render.sh`.

Known host condition: on this Mac, `maps.sh` gave rc 141 (SIGPIPE) on 2026-09-26 on the UNCHANGED chart, because a new pipe held too few bytes (`ci/helm-render/README.md`, residual risk 5). The gate then prints `FAIL  [6 maps.sh]: infrastructure error (rc=2)` and `gate rc=2`. This change does not touch `maps.sh`. If you see exactly that, and every other row passes, record it and rely on CI's `repo:helm-render` for `maps.sh`. Any other failure is a defect.

- [ ] **Step 2: Run the Moon target**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:helm-render; echo "moon rc=$?"
```

Expected: `moon rc=0`. Moon runs the three modes of Step 1 with the `bash` on `PATH`. The same `maps.sh` host condition as Step 1 applies. If this target fails, follow the diagnosis procedure in the root `CLAUDE.md` (Step 0: copy `.moon/cache/ciReport.json` and `.moon/cache/states/repo/helm-render/` first).

- [ ] **Step 3: Run the actionlint gate for `ci/kind/run.sh` (check 13) if the host allows it**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
/opt/homebrew/bin/bash ci/actionlint/run.sh; echo "actionlint rc=$?"
```

(Check 13 reads the git index. `ci/kind/run.sh` is committed in Task 4, so the index holds the new version.) Expected: the preflight line reports a pipe capacity at or above the 8192-byte floor, then the gate finishes in about 47 s with `actionlint rc=0` and no check 13 FAIL. If the preflight reports `small` and rc 2, there is no local verdict (root `CLAUDE.md`, "This development Mac only"). Record it and rely on CI's `repo:actionlint`.

- [ ] **Step 4: Check the file set of the branch**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-691-chart-audience
git diff --stat main...HEAD
git log --oneline main..HEAD
```

Expected: exactly these files change (plus the spec and this plan, from the commits before Task 1):
`charts/paigasus/README.md`, `charts/paigasus/templates/NOTES.txt`, `charts/paigasus/templates/_audience.tpl`, `charts/paigasus/templates/backend-deployment.yaml`, `charts/paigasus/tests/env.sh`, `charts/paigasus/tests/golden/iam-and-gateway.yaml`, `charts/paigasus/tests/golden/iam-only.yaml`, `charts/paigasus/values.yaml`, `ci/kind/README.md`, `ci/kind/run.sh`, `docs/ops/RUNBOOK-chart.md`, `docs/superpowers/specs/2026-09-26-sma-691-measurements.md`. NOT changed: `charts/paigasus/templates/_helpers.tpl`, anything under `ci/helm-render/`, `ci/kind/values/`, `ci/affected-graph/`, `.github/`.

- [ ] **Step 5: Before the push (the controller, at the PR stage)**

- Run the full `moon ci` target list from the root `CLAUDE.md` ("Before you push: the full gate graph"), with the bash split that the root `CLAUDE.md` describes for this Mac.
- File the follow-up Linear issue of spec § 8 (the console cannot request an API audience from Auth0 or Entra ID). Set project, milestone ("IAM Gaps") and priority at creation.
- AC 2 proof: `.github/workflows/chart.yml` runs on this PR because `charts/**` and `ci/kind/**` are in its `pull_request` path filter. The PR description links the green `chart.yml` run. Its `install a` and `upgrade b` logs show `install a: NOTES shows the SMA-691 audience warning` and `upgrade b: NOTES shows the SMA-691 audience warning`. The kind job does not run locally in this plan.

---

## Notes: measured while writing this plan (2026-09-26)

All measurements used a scratch copy of the chart (`cp -R charts/paigasus <scratchpad>/chart`), the pinned helm binary `~/.proto/tools/helm/3.22.0/darwin-arm64/helm` (`v3.22.0+g144ca65`), `/bin/bash` 3.2.57 and `/opt/homebrew/bin/bash` 5.3.15. No repository file other than this plan changed.

- The exact snippets of Tasks 1-3 render as intended. The default render shows the annotation directly under `metadata.name` of `paigasus-paigasus-iam-backend`. With `oidc.audience=api://x` the Deployment metadata renders as before.
- Byte identity: against the unchanged chart, the full render differs ONLY by the two annotation lines for the gateway-off, gateway-on and `oidc.audience=null` inputs, and not at all for `oidc.audience=12345` and `oidc.audience=api://default` (no warning in those two).
- Probe results (`warns` / NOTES): default → warns; `ack=null` → warns, `toString` gives `<nil>`; `audience=paigasus-console` → warns; `audience=api://paigasus` → empty; `ack=paigasus-console` → empty; `ack=true` (bool) → warns; `--set-string ack=false` → warns; `ack=old-client` → warns; `ack: ~` in a values file → `<nil>`, warns; `ack: 12345` in a file with `--set oidc.clientId=12345` → empty; `ack: 1000000` in a file with `--set-string oidc.clientId=1000000` → `1e+06`, warns; `ack= paigasus-console` (leading blank) and `ack=PAIGASUS-CONSOLE` → warns.
- The trimmed `NOTES.txt` renders an EMPTY string when there is no warning (the probe that wraps its bytes gives `notes: ""`).
- `helm template` fails (rc 1) when `NOTES.txt` has a template error; `helm lint` fails too. `helm install --dry-run=client` with no cluster fails with `Kubernetes cluster unreachable`.
- The full scratch `env.sh` (A1-A6, W1-W14, N0-N5) passes under both bashes. The failing-first outputs of Task 2 Step 3 and Task 3 Step 3 were measured on scratch copies in the before state.
- All other chart scripts on the scratch chart: `ca-bundle.sh`, `ingress.sh`, `names.sh` pass; `refusals.sh` passes when it gets `--set ingress.host=…` as `repo:helm-render` passes it (without it, its two positive rows fail on the unchanged chart too); `maps.sh` exits 141 on this host on the unchanged chart as well.
- The mutation battery of Task 6 was measured on the scratch copy; the "Expected red rows" column is that result.

## Spec coverage

| Spec item | Task |
|---|---|
| § 3 D1 no behavior change, byte identity | Task 1 Steps 4-5; Task 2 Step 8 |
| § 3 D2 two signals | Tasks 2 (annotation), 3 (NOTES) |
| § 3 D3 condition "audience equals client id" | Task 2 Step 5; rows W3, N2 |
| § 3 D4 acknowledgement names the client id | Task 2 Steps 4-5; rows W5-W8, W10-W12, N4, N5 |
| § 3 D5 new helper file, `_helpers.tpl` unchanged | Tasks 1-3; Task 7 Step 4 |
| § 3 D6 NOTES tested offline and in kind | Task 3 (N0-N5); Task 4 |
| § 3 D7 text states only what the chart knows | Task 3 Step 4 (spec text verbatim) |
| § 4.1 values key and nil handling | Task 2 Step 4; row W2 |
| § 4.2 helpers, root context | Tasks 1-3 Interfaces |
| § 4.3 NOTES.txt and body | Task 3 Steps 4-5 |
| § 4.4 annotation, metadata only, Go comment | Task 2 Step 6; rows W1, W9, W14; mutation M10 |
| § 4.5 documents | Task 5 (values, runbook, README); Tasks 2-3 (env.sh header); Task 4 (`ci/kind/README.md`) |
| § 4.6 migration order | Task 5 Step 6 |
| § 5.1 rows W1-W9, "absent" rows check the Deployment exists | Task 2 Step 2 |
| § 5.2 rows N0-N5, row counter | Task 3 Step 2 |
| § 5.3 A1-A6 unchanged | Task 1 Step 4 |
| § 5.4 goldens, two lines per file | Task 2 Step 8 |
| § 5.5 kind check in `install_a` and `upgrade_b` | Task 4 |
| § 5.6 mutation table | Task 6 |
| § 6 rollout (no pod restart) | row W14; Task 5 Step 4 |
| § 8 follow-up issue | Task 7 Step 5 |
| AC 1 | Tasks 2-3 |
| AC 2 | Task 7 Step 5 (the PR's `chart.yml` run) |
| AC 3 | Task 5 Step 6 |

Additions beyond the spec, from the Review Focus: rows W10-W14 (so `WARNING_ROWS_WANT` is 14, not 9), mutations M10-M15, the kind harness, and the probe that wraps the bytes of `NOTES.txt` (Task 3 design note).

Corrections to spec § 5.6, both MEASURED: M5 ("drop `eq $id "iam"` → W1 count") and M9a ("drop `toString` → A4, A5") are equivalent mutants after this design. Task 6 records them and adds M9b in place of M9a.
