# SMA-678 oidc.audience Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the optional chart value `oidc.audience`. When it is set, it REPLACES `oidc.clientId` as the one audience in IAM's `IAM_AUTHN__ISSUERS`. When it is empty or absent, the render stays byte-identical to `main`.

**Architecture:** One template line in `charts/paigasus/templates/backend-deployment.yaml` changes to `($root.Values.oidc.audience | default $root.Values.oidc.clientId | toString)`. A Go template comment (it does not render) records the design. A conditional YAML comment line renders only when the value is set. `charts/paigasus/tests/env.sh` gets six rows (A1-A6) and a row counter. One `figment::Jail` unit test in `paigasus-iam` pins IAM's parse of the rendered string. The runbook and the chart README document the value. No production Rust code changes.

**Tech Stack:** Helm 3.22.0 (proto-pinned), bash (must run under `/bin/bash` 3.2.57 and bash 5), Python 3 with PyYAML (inline in the chart scripts), Rust edition 2024 with figment 0.10.19 and cargo-nextest, Moon 2.5.3 through proto shims.

**Spec:** docs/superpowers/specs/2026-09-24-sma-678-oidc-audience-design.md

## Global Constraints

- Worktree: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-678-oidc-audience`, branch `feature/sma-678-oidc-audience`. A subagent starts in the MAIN checkout. It must `cd` to the worktree and run `git branch --show-current` first. The output must be `feature/sma-678-oidc-audience`.
- Every command block starts with `cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-678-oidc-audience` and `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Run every `helm` and chart-script command from the worktree root, or from a directory under it. MEASURED: outside the repository the proto shim finds no `.prototools` and runs the global helm (v4.3.0 on this Mac). `render.sh` then fails with a blank line before every `---`. That failure is not caused by this change.
- The chart scripts call `python3`, which must import `yaml`. Check with `python3 -c 'import yaml'`. `repo:helm-render` puts its own venv first on `PATH`.
- Every new source file opens with an SPDX header. This plan adds no new source file.
- `charts/paigasus/tests/env.sh` must stay safe under `/bin/bash` 3.2.57: no `mapfile`, no `declare -A`, no here-string, no pipe into a reader that exits early. Keep the `"${BASE[@]+"${BASE[@]}"}"` guard. Inside a `python3 -c '…'` block use only DOUBLE quotes.
- The golden files `charts/paigasus/tests/golden/iam-only.yaml` and `charts/paigasus/tests/golden/iam-and-gateway.yaml` must not change. Never run `render.sh --update`.
- Do not change `charts/paigasus/templates/backend-deployment.yaml` lines 97-99 (the three YAML comment lines above `IAM_AUTHN__ISSUERS`). They render into the goldens.
- Do not change the eight stub-value copies, `ci/kind/values/a.yaml`, `charts/CLAUDE.md`, `templates/_helpers.tpl`, or any file under `ci/helm-render/` (spec D4, D9, § 3.7).
- Commit scope must be one of `rs`, `py`, `ts`, `contracts`, `ci`, `docs`, `deps`, `release`, `repo`, `claude`, `workspace`. `charts` is NOT a valid scope. Use `repo` for the chart and the docs, and `rs` for the Rust test. Header max 100 characters. Body lines max 100 characters.
- Commit subjects:
  - Before Task 1 (the controller): `docs(repo): plan for the oidc.audience chart value (SMA-678)`
  - Task 1: `feat(repo): add the oidc.audience chart value for IAM (SMA-678)`
  - Task 2: `test(rs): pin IAM's parse of the chart's oidc.audience issuer string (SMA-678)`
  - Task 3: `docs(repo): document oidc.audience in the chart runbook and README (SMA-678)`
- Every commit message ends with the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`, after one blank line.
- No body line starts with `#` followed by a number. No body line has the form `Token: value` (a word, a colon, a space). Only the trailer has that form. Such lines fail `footer-leading-blank`.
- Never use `--no-verify`. If `commitlint` is not found, provision the worktree (`proto install`, then `pnpm -C ts install`) and commit again.
- Never use `git commit --amend`. Never use `git reset`. Never use `git stash`. Each task makes one new commit.
- Remove every mutation with the Edit tool (the inverse Edit). Never use `git checkout --` or `git restore`: they also discard the uncommitted change under test.
- Stage exact paths with `git add <path>`. Never `git add -A` or `git add .`.
- If the sandbox refuses the HEREDOC commit form, write the message to a file in the session scratchpad and run `git commit -F <that file>`.
- Write every new comment, doc text and commit message in ASD-STE100 Simplified Technical English. Keep technical names as they are.

## Review Focus

1. **A number from a values file.** `--set oidc.audience=12345` gives an int64 (spec row A4). An unquoted `audience: 12345` in a values file gives a float64, which is a different type. Without `toString`, `%q` writes `%!q(float64=12345)`. MEASURED: with `toString` it renders `"12345"`. Task 1 adds row `A5 number-in-file`, and mutation M3 must red A4 and A5.
2. **The runbook's restart claim.** Task 3 writes into runbook § 5 that a change of `oidc.audience` restarts the IAM pod and no console pod. No spec row tests that. Task 1 adds row `A6 restart-scope`: the IAM pod template must differ and both console pod templates must be equal. Mutation M7 (the audience fed into a console checksum) must red A6.
3. **A digit-only audience at IAM's parser.** The chart quotes the audience with `%q`. MEASURED with figment 0.10.19: `audiences=["12345"]` parses to the string `"12345"`, but the unquoted `audiences=[12345]` fails with `invalid type: … expected a string`. Task 2 tests both the URI form and the digit form, and mutation M-rs1 removes the quotes to prove the test bites.
4. **The whitespace control of the Go template comment.** The comment must start with `{{- /*`. MEASURED: `{{/*` (no left trim) adds one blank line to the IAM Deployment. `env.sh` stays green, because a blank line is valid YAML. Only `render.sh` (the goldens) catches it. Task 1 mutation M8 proves that the golden check reds.
5. **A null in a user values file** (`oidc: {audience: ~}`). MEASURED: Helm deletes the key, the chart default `""` applies, and the render gives `["paigasus-console"]`. That is the A1 path, not a new one. No extra test is necessary.

---

### Task 1: The chart value, the template line and the env.sh rows

**Files:**
- Modify: `charts/paigasus/tests/env.sh` (header lines 15-18; the tail at lines 92-96)
- Modify: `charts/paigasus/values.yaml` (after line 80, `clientId`)
- Modify: `charts/paigasus/templates/backend-deployment.yaml` (insert before line 100; change line 101)

**Interfaces:**
- New value `oidc.audience` (string, default `""`, NOT required).
- New shell functions in `env.sh`: `check_audience <label> <expected audience> <present|absent> [helm args...]` and `check_audience_restart <label>`. The third argument of `check_audience` is the expected state of the YAML comment line `            # oidc.audience is set: IAM accepts that audience, not the client id.` (12 spaces of indent).
- Row counter: `AUDIENCE_ROWS` must reach `AUDIENCE_ROWS_WANT=6`.

- [ ] **Step 1: Write the failing rows in env.sh (header)**

Use the Edit tool on `charts/paigasus/tests/env.sh`.

old_string:
```bash
# The ten keys below are the ones @paigasus/auth, @paigasus/discovery and each app's own
# lib/config.ts declare with no default. Anything missing here is a pod that fails its
# configuration parse on the first request.
set -euo pipefail
```

new_string:
```bash
# The ten keys below are the ones @paigasus/auth, @paigasus/discovery and each app's own
# lib/config.ts declare with no default. Anything missing here is a pod that fails its
# configuration parse on the first request.
#
# The script also checks the IAM backend's IAM_AUTHN__ISSUERS value (SMA-678, check_audience).
# One row per property of oidc.audience:
#   A1 unset        the audience is oidc.clientId, and the "oidc.audience is set" comment is absent.
#   A2 reuse-values-no-key
#                   `helm upgrade --reuse-values` from a release made before the key existed: the
#                   template reads nil, and the audience is still oidc.clientId.
#   A3 set          the value REPLACES oidc.clientId. The exact compare proves a list of one.
#   A4 number       an int64 from --set renders as the string "12345", not a rune literal.
#   A5 number-in-file
#                   a number in a values file (a float64) renders as the string "12345" too.
#   A6 restart-scope
#                   a change of the value changes the IAM pod template and no console pod template.
# A row counter reds the script when a row call line is deleted.
set -euo pipefail
```

- [ ] **Step 2: Write the failing rows in env.sh (functions and rows)**

Use the Edit tool on `charts/paigasus/tests/env.sh`.

old_string:
```bash
check "iam only"        --set zones.gateway.enabled=false
check "iam and gateway" --set zones.gateway.enabled=true

if [ "$ec" -eq 0 ]; then echo "== chart env OK =="; fi
```

new_string:
```bash
check "iam only"        --set zones.gateway.enabled=false
check "iam and gateway" --set zones.gateway.enabled=true

# oidc.audience (SMA-678). Renders go to a file, not through a pipe: a Linux runner holds the whole
# render in its pipe, a 512-byte host pipe does not (ci/helm-render/README.md, residual risk 5).
TMP="$(mktemp -d "${TMPDIR:-/tmp}/env.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT
AUDIENCE_ROWS=0
AUDIENCE_ROWS_WANT=6

# check_audience <label> <expected audience> <present|absent> [helm args...]
# The third argument is the state of the "oidc.audience is set" YAML comment line.
check_audience() {
  local label="$1" want="$2" comment="$3"; shift 3
  local out
  AUDIENCE_ROWS=$((AUDIENCE_ROWS + 1))
  if ! helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" "$@" >"$TMP/audience.yaml" 2>"$TMP/audience.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/audience.err"; ec=1; return 0
  fi
  if ! out="$(WANT="$want" COMMENT="$comment" python3 -c '
import os, sys, yaml
want = "[{issuer=\"https://idp.example.test/realms/paigasus\",audiences=[\"" + os.environ["WANT"] + "\"]}]"
line = "            # oidc.audience is set: IAM accepts that audience, not the client id."
with open(sys.argv[1]) as fh:
    raw = fh.read()
docs = [d for d in yaml.safe_load_all(raw) if d]
problems = []
deps = [d for d in docs if d.get("kind") == "Deployment"
        and d["spec"]["template"]["metadata"]["labels"].get("app.kubernetes.io/name") == "iam-backend"]
if len(deps) != 1:
    problems.append(str(len(deps)) + " iam-backend Deployment(s), want 1")
else:
    env = deps[0]["spec"]["template"]["spec"]["containers"][0].get("env") or []
    issuers = [e for e in env if e.get("name") == "IAM_AUTHN__ISSUERS"]
    if len(issuers) != 1:
        problems.append(str(len(issuers)) + " IAM_AUTHN__ISSUERS entries, want 1")
    elif issuers[0].get("value") != want:
        problems.append("IAM_AUTHN__ISSUERS is " + repr(issuers[0].get("value")) + ", want " + repr(want))
count = raw.splitlines().count(line)
if os.environ["COMMENT"] == "present" and count != 1:
    problems.append("the oidc.audience comment line renders " + str(count) + " time(s), want 1")
if os.environ["COMMENT"] == "absent" and count != 0:
    problems.append("the oidc.audience comment line renders " + str(count) + " time(s), want 0")
print("|".join(problems) if problems else "OK")' "$TMP/audience.yaml" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

# check_audience_restart <label>: a change of oidc.audience restarts the IAM pod and no console pod
# (docs/ops/RUNBOOK-chart.md § 5). The gateway zone is on, so both consoles are in the render.
check_audience_restart() {
  local label="$1" out
  AUDIENCE_ROWS=$((AUDIENCE_ROWS + 1))
  if ! helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" --set zones.gateway.enabled=true >"$TMP/restart-1.yaml" 2>"$TMP/restart.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/restart.err"; ec=1; return 0
  fi
  if ! helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" --set zones.gateway.enabled=true --set oidc.audience=api://default >"$TMP/restart-2.yaml" 2>"$TMP/restart.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/restart.err"; ec=1; return 0
  fi
  if ! out="$(python3 -c '
import sys, yaml
def templates(path):
    with open(path) as fh:
        docs = [d for d in yaml.safe_load_all(fh) if d]
    return {d["spec"]["template"]["metadata"]["labels"]["app.kubernetes.io/name"]: d["spec"]["template"] for d in docs if d.get("kind") == "Deployment"}
a, b = templates(sys.argv[1]), templates(sys.argv[2])
want = ["gateway-console", "iam-backend", "iam-console"]
problems = []
if sorted(a) != want or sorted(b) != want:
    problems.append("Deployments are " + repr(sorted(a)) + " and " + repr(sorted(b)) + ", want " + repr(want))
elif a["iam-backend"] == b["iam-backend"]:
    problems.append("iam-backend: spec.template is equal; it must differ")
problems += [n + ": spec.template differs; it must be equal" for n in ("gateway-console", "iam-console") if n in a and n in b and a[n] != b[n]]
print("|".join(problems) if problems else "OK")' "$TMP/restart-1.yaml" "$TMP/restart-2.yaml" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

# A5: a number in a VALUES FILE is a float64, not the int64 that --set gives (Review Focus 1).
printf 'oidc:\n  audience: 12345\n' >"$TMP/audience-number.yaml"

check_audience "A1 unset"               paigasus-console absent
check_audience "A2 reuse-values-no-key" paigasus-console absent  --set oidc.audience=null
check_audience "A3 set"                 api://default    present --set oidc.audience=api://default
check_audience "A4 number"              12345            present --set oidc.audience=12345
check_audience "A5 number-in-file"      12345            present -f "$TMP/audience-number.yaml"
check_audience_restart "A6 restart-scope"

if [ "$AUDIENCE_ROWS" -lt "$AUDIENCE_ROWS_WANT" ]; then
  echo "FAIL [audience rows]: $AUDIENCE_ROWS check_audience row(s) ran, want $AUDIENCE_ROWS_WANT"; ec=1
fi

if [ "$ec" -eq 0 ]; then echo "== chart env OK =="; fi
```

- [ ] **Step 3: Run env.sh and see it fail**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-678-oidc-audience
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
/bin/bash charts/paigasus/tests/env.sh; echo "rc=$?"
```

Expected (measured on a scratch copy of the chart): `ok [iam only]`, `ok [iam and gateway]`, `ok [A1 unset]`, `ok [A2 reuse-values-no-key]`, then four FAIL rows, and `rc=1`:

```text
FAIL [A3 set]: IAM_AUTHN__ISSUERS is '[{issuer="https://idp.example.test/realms/paigasus",audiences=["paigasus-console"]}]', want '[{issuer="https://idp.example.test/realms/paigasus",audiences=["api://default"]}]'|the oidc.audience comment line renders 0 time(s), want 1
FAIL [A4 number]: IAM_AUTHN__ISSUERS is '[…audiences=["paigasus-console"]}]', want '[…audiences=["12345"]}]'|the oidc.audience comment line renders 0 time(s), want 1
FAIL [A5 number-in-file]: IAM_AUTHN__ISSUERS is '[…audiences=["paigasus-console"]}]', want '[…audiences=["12345"]}]'|the oidc.audience comment line renders 0 time(s), want 1
FAIL [A6 restart-scope]: iam-backend: spec.template is equal; it must differ
```

A1 and A2 pass before the change. That is correct: they pin the default path, which must not change.

- [ ] **Step 4: Add the value to values.yaml**

Use the Edit tool on `charts/paigasus/values.yaml`.

old_string:
```yaml
  clientId: ""           # REQUIRED
  existingSecret: ""     # REQUIRED. Must hold keys: oidc-client-secret, session-redis-url.
```

new_string:
```yaml
  clientId: ""           # REQUIRED
  audience: ""           # NOT required. The access-token audience that IAM accepts. Empty means
                         # oidc.clientId. Set it only when your IdP puts a different value in the
                         # access token's `aud` claim (for example Okta: api://default). It REPLACES
                         # oidc.clientId; IAM then does not accept oidc.clientId as the audience.
                         # Quote it in a values file. See docs/ops/RUNBOOK-chart.md § 6.
  existingSecret: ""     # REQUIRED. Must hold keys: oidc-client-secret, session-redis-url.
```

- [ ] **Step 5: Change the template**

Use the Edit tool on `charts/paigasus/templates/backend-deployment.yaml`. Lines 97-99 stay byte-identical: they are context in old_string and new_string.

old_string:
```yaml
            # the access tokens IAM validates were issued to.
            - name: IAM_AUTHN__ISSUERS
              value: {{ printf "[{issuer=%q,audiences=[%q]}]" $root.Values.oidc.issuer $root.Values.oidc.clientId | quote }}
```

new_string:
```yaml
            # the access tokens IAM validates were issued to.
{{- /*
  oidc.audience (SMA-678). Empty, or absent (nil under `helm upgrade --reuse-values` from a
  release made before the key existed): the audience is oidc.clientId, and the render is
  byte-identical to the render before the value existed. Set: the value REPLACES oidc.clientId.
  It is not added next to it, so IAM refuses a token whose aud holds only the client id (an ID
  token among them). toString runs before %q: %q on a number from --set or from a values file
  writes a rune literal or an error string, not the digits.
*/}}
{{- if $root.Values.oidc.audience }}
            # oidc.audience is set: IAM accepts that audience, not the client id.
{{- end }}
            - name: IAM_AUTHN__ISSUERS
              value: {{ printf "[{issuer=%q,audiences=[%q]}]" $root.Values.oidc.issuer ($root.Values.oidc.audience | default $root.Values.oidc.clientId | toString) | quote }}
```

The comment opens with `{{- /*` (left trim) and closes with `*/}}` (no right trim), as spec § 3.2 says.

- [ ] **Step 6: Run env.sh and render.sh and see them pass, under both bash versions**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-678-oidc-audience
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
/bin/bash charts/paigasus/tests/env.sh; echo "rc=$?"
/opt/homebrew/bin/bash charts/paigasus/tests/env.sh; echo "rc=$?"
/bin/bash charts/paigasus/tests/render.sh; echo "rc=$?"
/bin/bash charts/paigasus/tests/ca-bundle.sh; echo "rc=$?"
git diff --exit-code -- charts/paigasus/tests/golden; echo "golden rc=$?"
```

Expected: each `env.sh` run prints `ok` for `iam only`, `iam and gateway` and `A1`-`A6`, then `== chart env OK ==` and `rc=0`. `render.sh` prints `ok [iam-only]`, `ok [iam-and-gateway]`, `== chart render OK ==`, `rc=0`. `ca-bundle.sh` prints `== chart ca-bundle OK ==`, `rc=0`. `golden rc=0` with no diff output.

If a Homebrew bash run hangs at about 0% CPU, stop it. That is the host pipe condition (SMA-612), not a defect of this change. Record it and rely on the `/bin/bash` result.

- [ ] **Step 7: Prove every row bites (the mutation battery)**

Apply each mutation with the Edit tool, run the command below, compare with the expected red rows, then remove the mutation with the inverse Edit. Do one mutation at a time. The "red rows" column is MEASURED on a scratch copy of the chart.

The command for each mutation:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-678-oidc-audience
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
/bin/bash charts/paigasus/tests/env.sh; echo "rc=$?"
```

For M6 and M8, also run `/bin/bash charts/paigasus/tests/render.sh; echo "rc=$?"`.

| # | File | old_string (exact) | new_string (exact) | Red rows (rc=1) |
|---|---|---|---|---|
| M1 | `templates/backend-deployment.yaml` | `($root.Values.oidc.audience \| default $root.Values.oidc.clientId \| toString)` | `$root.Values.oidc.clientId` | A3, A4, A5, A6 |
| M2 | `templates/backend-deployment.yaml` | `($root.Values.oidc.audience \| default $root.Values.oidc.clientId \| toString)` | `$root.Values.oidc.audience` | A1 (`[""]`), A2 (`[%!q(<nil>)]`), A4, A5 |
| M3 | `templates/backend-deployment.yaml` | `($root.Values.oidc.audience \| default $root.Values.oidc.clientId \| toString)` | `($root.Values.oidc.audience \| default $root.Values.oidc.clientId)` | A4 (`['〹']`), A5 |
| M4 | `templates/backend-deployment.yaml` | `printf "[{issuer=%q,audiences=[%q]}]" $root.Values.oidc.issuer ($root.Values.oidc.audience` | `printf "[{issuer=%q,audiences=[%q,%q]}]" $root.Values.oidc.issuer $root.Values.oidc.clientId ($root.Values.oidc.audience` | A1, A2, A3, A4, A5 |
| M5a | `tests/env.sh` | the whole line `check_audience "A4 number"              12345            present --set oidc.audience=12345` plus its newline | (empty) | `FAIL [audience rows]: 5 check_audience row(s) ran, want 6` |
| M5b | `tests/env.sh` | the whole line `check_audience_restart "A6 restart-scope"` plus its newline | (empty) | `FAIL [audience rows]: 5 check_audience row(s) ran, want 6` |
| M6 | `templates/backend-deployment.yaml` | the three lines `{{- if $root.Values.oidc.audience }}`, `            # oidc.audience is set: IAM accepts that audience, not the client id.`, `{{- end }}` | the one line `            # oidc.audience is set: IAM accepts that audience, not the client id.` | env.sh: A1, A2 (`renders 1 time(s), want 0`); render.sh: both goldens |
| M7 | `templates/console-deployment.yaml` | `checksum/secret: {{ $root.Values.oidc.secretVersion \| sha256sum }}` | `checksum/secret: {{ print $root.Values.oidc.secretVersion $root.Values.oidc.audience \| sha256sum }}` | A6 (`gateway-console: spec.template differs; it must be equal\|iam-console: …`) |
| M8 | `templates/backend-deployment.yaml` | `{{- /*` | `{{/*` | env.sh stays green (rc=0); render.sh: `FAIL [iam-only]` and `FAIL [iam-and-gateway]` (one added blank line) |

The `\|` in the table is a Markdown escape. The real character in each old_string and new_string is `|`.

M1 is the spec's "line 101 uses `oidc.clientId` again". M2 is the spec's "remove `default`". M3 is "remove `toString`". M4 is "add the audience next to the client id". M5a and M5b are "delete one `check_audience` call line". M6, M7 and M8 are additions from this plan's Review Focus (items 2 and 4) and from spec D6.

After the last inverse Edit, run Step 6 again. All rows must pass, and `git diff --stat` must list only the three files of this task.

- [ ] **Step 8: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-678-oidc-audience
git add charts/paigasus/values.yaml charts/paigasus/templates/backend-deployment.yaml charts/paigasus/tests/env.sh
git status --short
git commit -m "$(cat <<'EOF'
feat(repo): add the oidc.audience chart value for IAM (SMA-678)

The optional value oidc.audience replaces oidc.clientId as the one audience in
IAM_AUTHN__ISSUERS. Empty or absent, the chart uses oidc.clientId and the render
is byte-identical to main. toString keeps a number from --set or a values file a
string. env.sh adds rows A1-A6 and a row counter; each named mutation reds its row.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
git log --oneline -1
```

Expected: `git status --short` shows only the three `M` lines before the commit. The log shows the new commit.

---

### Task 2: IAM parses the rendered issuer string

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/config.rs` (test module; insert after `authn_defaults_land_with_a_minimal_issuer`, which ends near line 1478, and before `missing_issuers_is_a_load_error`)

**Interfaces:**
- New unit test `issuers_env_in_the_chart_form_parses_a_uri_and_a_digit_audience`. It uses the existing helper `valid_pepper_b64()` (same module, near line 1995) and `IamConfig::figment()`.

No production code changes, so this test passes at once. It is a contract test between the chart's string and IAM's parser (spec § 3.5). Step 3 proves that it bites.

- [ ] **Step 1: Write the test**

Use the Edit tool on `rs/crates/services/paigasus-iam/src/config.rs`.

old_string:
```rust
            assert!(cfg.validate().is_ok(), "a single valid issuer should pass validation");
            Ok(())
        });
    }

    #[test]
    fn missing_issuers_is_a_load_error() {
```

new_string:
```rust
            assert!(cfg.validate().is_ok(), "a single valid issuer should pass validation");
            Ok(())
        });
    }

    #[test]
    fn issuers_env_in_the_chart_form_parses_a_uri_and_a_digit_audience() {
        // SMA-678: the exact strings that charts/paigasus renders into IAM_AUTHN__ISSUERS for
        // oidc.audience=api://default (env.sh row A3) and oidc.audience=12345 (rows A4 and A5).
        // The chart quotes each audience with %q. Without the quotes figment reads 12345 as a
        // number, and the extract fails.
        for (issuers, want) in [
            (r#"[{issuer="https://idp.example.test/realms/paigasus",audiences=["api://default"]}]"#, "api://default"),
            (r#"[{issuer="https://idp.example.test/realms/paigasus",audiences=["12345"]}]"#, "12345"),
        ] {
            figment::Jail::expect_with(|jail| {
                jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
                jail.set_env("IAM_API_KEYS__PEPPER", valid_pepper_b64());
                jail.set_env("IAM_AUTHN__ISSUERS", issuers);
                let cfg: IamConfig = IamConfig::figment().extract()?;
                assert_eq!(cfg.authn.issuers.len(), 1);
                assert_eq!(cfg.authn.issuers[0].issuer, "https://idp.example.test/realms/paigasus");
                assert_eq!(cfg.authn.issuers[0].audiences, vec![want.to_string()]);
                assert!(cfg.validate().is_ok(), "the chart's issuer string must pass validation");
                Ok(())
            });
        }
    }

    #[test]
    fn missing_issuers_is_a_load_error() {
```

- [ ] **Step 2: Run the test and see it pass**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-678-oidc-audience/rs
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo nextest run --locked -p paigasus-iam --lib -E 'test(issuers_env_in_the_chart_form)'; echo "rc=$?"
```

Expected: `PASS [...] paigasus-iam config::tests::issuers_env_in_the_chart_form_parses_a_uri_and_a_digit_audience`, a summary of `1 test run: 1 passed`, and `rc=0`. `--lib` runs only the unit tests, so no Docker is necessary.

MEASURED before this plan, in a scratch crate on figment 0.10.19 with the same `Env::prefixed("IAM_").split("__")` provider: both strings parse to `["api://default"]` and `["12345"]`.

- [ ] **Step 3: Prove the test bites (mutation M-rs1)**

Use the Edit tool on `rs/crates/services/paigasus-iam/src/config.rs`.

old_string:
```rust
audiences=["12345"]}]"#, "12345"),
```

new_string:
```rust
audiences=[12345]}]"#, "12345"),
```

Run:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-678-oidc-audience/rs
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo nextest run --locked -p paigasus-iam --lib -E 'test(issuers_env_in_the_chart_form)'; echo "rc=$?"
```

Expected: the test FAILS and `rc` is not 0. The panic text names an `invalid type` for `12345` and `expected a string`, at the key `authn.issuers.0.audiences.0`. The mutation compiles, so the failure is the assertion path and not a compile error.

Remove the mutation with the inverse Edit (old_string `audiences=[12345]}]"#, "12345"),`, new_string `audiences=["12345"]}]"#, "12345"),`). Run the Step 2 command again: `1 passed`, `rc=0`.

- [ ] **Step 4: Format and lint the crate**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-678-oidc-audience/rs
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo fmt --check -p paigasus-iam; echo "fmt rc=$?"
cargo clippy --locked -p paigasus-iam --all-targets -- -D warnings; echo "clippy rc=$?"
```

Expected: `fmt rc=0` and `clippy rc=0`. `rs/rustfmt.toml` sets `max_width = 200`, so the two tuple lines stay on one line each. If `cargo fmt --check` prints a diff, run `cargo fmt -p paigasus-iam`, read the diff, and run the check again.

- [ ] **Step 5: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-678-oidc-audience
git add rs/crates/services/paigasus-iam/src/config.rs
git status --short
git commit -m "$(cat <<'EOF'
test(rs): pin IAM's parse of the chart's oidc.audience issuer string (SMA-678)

A figment Jail test sets IAM_AUTHN__ISSUERS to the exact strings that the chart
renders for oidc.audience=api://default and oidc.audience=12345. IAM parses them
to one string audience each and passes validate().

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
git log --oneline -1
```

---

### Task 3: The runbook and the chart README

**Files:**
- Modify: `docs/ops/RUNBOOK-chart.md` (§ 1 table line 25; § 5 table after line 83; § 6 item 1 at lines 95-97; § 6 after line 125)
- Modify: `charts/paigasus/README.md` (new section after the `oidc.caBundle` section, which ends at line 113)

**Interfaces:** none (documentation only).

- [ ] **Step 1: Values table (§ 1)**

Use the Edit tool on `docs/ops/RUNBOOK-chart.md`.

old_string:
```markdown
| `oidc.clientId` | yes | The console's OIDC client. IAM also uses it as the access-token audience (§ 6) |
```

new_string:
```markdown
| `oidc.clientId` | yes | The console's OIDC client. IAM also uses it as the access-token audience, unless `oidc.audience` is set (§ 6) |
| `oidc.audience` | no | The access-token audience IAM accepts. Default: `oidc.clientId`. Set it only when the IdP puts another value in `aud` (§ 6) |
```

- [ ] **Step 2: What restarts what (§ 5)**

Use the Edit tool on `docs/ops/RUNBOOK-chart.md`.

old_string:
```markdown
| the contents of the CA ConfigMap | nothing, until you change `oidc.caBundle.version` | Node and IAM read the file once, at start |
```

new_string:
```markdown
| the contents of the CA ConfigMap | nothing, until you change `oidc.caBundle.version` | Node and IAM read the file once, at start |
| `oidc.audience` | the IAM pod, not the consoles | it changes `IAM_AUTHN__ISSUERS` in the IAM pod template. IAM has one replica and `maxSurge: 0` (`templates/backend-deployment.yaml`), so IAM is not available during the restart |
```

- [ ] **Step 3: The audience item (§ 6 item 1)**

Use the Edit tool on `docs/ops/RUNBOOK-chart.md`.

old_string:
```markdown
1. **Audience.** The access token's `aud` claim must contain `oidc.clientId`. The chart sets IAM's
   accepted audience to `oidc.clientId`, and no other value (an `oidc.audience` value is future
   work).
```

new_string:
```markdown
1. **Audience.** The access token's `aud` claim must contain the audience that IAM accepts. That
   is `oidc.audience` when it is set, and `oidc.clientId` when it is not. Set `oidc.audience` only
   when you cannot make the IdP put the client id into `aud`. The value replaces the client id; it
   is not added to it. Before you choose the value, decode a real access token and read its `aud`
   claim. The value helps only when the IdP issues a JWT access token for the console's scopes
   (`openid profile email offline_access`). The console sends no `audience` or `resource`
   parameter. So an IdP that then issues an opaque token, or a token for a different API, cannot
   work with this value. A wrong audience shows as a refused token in the IAM log
   (`ci/kind/README.md`, "Where to look first").
```

- [ ] **Step 4: The Okta note (§ 6, after the Keycloak example)**

Use the Edit tool on `docs/ops/RUNBOOK-chart.md`.

old_string:
```markdown
address and the `offline_access` role. The kind job's realm, `ci/kind/realm/paigasus-realm.json`,
is a complete example.
```

new_string:
```markdown
address and the `offline_access` role. The kind job's realm, `ci/kind/realm/paigasus-realm.json`,
is a complete example.

**Okta example. Not tested against a live Okta tenant.** An Okta authorization server puts its own
audience into the access token's `aud`, not the client id. The default authorization server uses
`api://default`.

1. Set `oidc.issuer` to the authorization-server issuer
   (`https://<org>.okta.com/oauth2/default`, or your custom server). Do not use the org
   authorization server (`https://<org>.okta.com`): other parties must not validate its access
   tokens.
2. Set `oidc.audience=api://default`, or the audience of your custom server. Quote the value in a
   values file.
3. Add an `email` claim (value `user.email`, included in the access token) to that authorization
   server. IAM needs `email` (item 2).

**Warning.** In Okta's default configuration, the access token's `sub` can be the user's login,
not the fixed user id. IAM keys the identity on the access token's `(iss, sub)`. If that is true
for your tenant, a login rename makes a new identity. Check `sub` in a real access token before
production use. This chart did not verify Okta's `sub` behaviour.
```

- [ ] **Step 5: The chart README section**

Use the Edit tool on `charts/paigasus/README.md`.

old_string:
```markdown
`docs/ops/RUNBOOK-chart.md` for the failure modes.

## The golden files
```

new_string:
```markdown
`docs/ops/RUNBOOK-chart.md` for the failure modes.

## The access-token audience (`oidc.audience`)

IAM accepts an access token only if its `aud` claim contains a configured audience. The chart
renders that list, with one element, into `IAM_AUTHN__ISSUERS` in
`templates/backend-deployment.yaml`.

- Empty or absent (the default): the audience is `oidc.clientId`. The render is byte-identical to
  a chart without the value. Under `helm upgrade --reuse-values` from an older release the key is
  absent, and the template reads nil; the result is the same.
- Set: the value REPLACES `oidc.clientId`. It is not added next to it. IAM then refuses a token
  whose `aud` holds only the client id, an ID token included.
- A number (`--set oidc.audience=12345`, or an unquoted number in a values file) renders as the
  string `"12345"`: the template applies `toString` before `%q`. Quote the value in a values file
  all the same, because a large number can become an exponent form.
- When the value is set, one more YAML comment line renders above `IAM_AUTHN__ISSUERS`:
  `# oidc.audience is set: IAM accepts that audience, not the client id.`
- A change of the value restarts the IAM pod and no console pod.

`tests/env.sh` holds the rows: `A1 unset`, `A2 reuse-values-no-key` (`--set oidc.audience=null`),
`A3 set`, `A4 number`, `A5 number-in-file` and `A6 restart-scope`. A row counter reds the script
when a row call line is deleted. See `docs/ops/RUNBOOK-chart.md` § 6 for when to set the value.

## The golden files
```

- [ ] **Step 6: Check the text**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-678-oidc-audience
grep -n 'future' docs/ops/RUNBOOK-chart.md; echo "future rc=$?"
grep -c 'oidc.audience' docs/ops/RUNBOOK-chart.md charts/paigasus/README.md
git diff --stat
```

Expected: `future rc=1` (the "future work" text is gone). `grep -c` counts lines: 5 for the runbook (the two table rows in § 1, the § 5 row, one line of item 1, Okta step 2) and 4 for the README. `git diff --stat` lists only the two files of this task.

- [ ] **Step 7: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-678-oidc-audience
git add docs/ops/RUNBOOK-chart.md charts/paigasus/README.md
git status --short
git commit -m "$(cat <<'EOF'
docs(repo): document oidc.audience in the chart runbook and README (SMA-678)

The runbook states when to set the value, that it replaces the client id, and
when it is not enough. It adds the restart row and an Okta note that is not
tested against a live tenant. The chart README describes the value and its rows.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
git log --oneline -1
```

---

### Task 4: Final verification

**Files:** none changed. If a step fails, fix the cause in the file of the task that owns it, and make a NEW commit with that task's scope. Do not amend.

**Interfaces:** none.

- [ ] **Step 1: The full repo:helm-render gate, under system bash 3.2**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-678-oidc-audience
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash ci/helm-render/run.sh --self-test; echo "self-test rc=$?"
/bin/bash ci/helm-render/run.sh --negative-control; echo "negative-control rc=$?"
/bin/bash ci/helm-render/run.sh; echo "full rc=$?"
```

Expected: three times `rc=0`. The full run prints `PASS  [6 env.sh]` and `== helm-render: all checks passed ==`. The gate runs every chart script as `"$BASH" <script>`, so this run also proves `env.sh` under bash 3.2. `ci/helm-render/README.md` states that the gate runs under bash 3.2 and bash 5 (measured 2026-09-22, pipe capacity 65536 bytes).

- [ ] **Step 2: The same gate under bash 5**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-678-oidc-audience
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/opt/homebrew/bin/bash ci/helm-render/run.sh --self-test; echo "self-test rc=$?"
/opt/homebrew/bin/bash ci/helm-render/run.sh --negative-control; echo "negative-control rc=$?"
/opt/homebrew/bin/bash ci/helm-render/run.sh; echo "full rc=$?"
```

Expected: three times `rc=0`. If a run hangs at about 0% CPU, stop it. That is the SMA-612 host pipe condition, not a finding. Record it, and use the Step 1 result.

- [ ] **Step 3: The goldens did not change**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-678-oidc-audience
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash charts/paigasus/tests/render.sh; echo "render rc=$?"
git diff --exit-code "$(git merge-base HEAD origin/main)" HEAD -- charts/paigasus/tests/golden; echo "golden rc=$?"
git diff --exit-code -- charts/paigasus/tests/golden; echo "worktree golden rc=$?"
```

Expected: `== chart render OK ==`, `render rc=0`, `golden rc=0`, `worktree golden rc=0`, and no diff output.

- [ ] **Step 4: The IAM crate**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-678-oidc-audience/rs
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo fmt --check -p paigasus-iam; echo "fmt rc=$?"
cargo clippy --locked -p paigasus-iam --all-targets -- -D warnings; echo "clippy rc=$?"
cargo nextest run --locked -p paigasus-iam --lib; echo "lib tests rc=$?"
```

Expected: three times `rc=0`. The lib run includes `issuers_env_in_the_chart_form_parses_a_uri_and_a_digit_audience` and needs no Docker.

- [ ] **Step 5: The branch state**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-678-oidc-audience
git status --short
git log --oneline "$(git merge-base HEAD origin/main)"..HEAD
git diff --stat "$(git merge-base HEAD origin/main)" HEAD
```

Expected: `git status --short` prints nothing. The log shows the spec commit, the plan commit and the three task commits. The diff stat lists exactly these files: the spec, the plan, `charts/paigasus/values.yaml`, `charts/paigasus/templates/backend-deployment.yaml`, `charts/paigasus/tests/env.sh`, `rs/crates/services/paigasus-iam/src/config.rs`, `docs/ops/RUNBOOK-chart.md` and `charts/paigasus/README.md`. No file under `charts/paigasus/tests/golden/`, `ci/` or `charts/CLAUDE.md`.

The full pre-push `moon ci` graph (root `CLAUDE.md`, "Before you push") is the job of the open-pr stage, not of this plan.

## Spec coverage

| Spec item | Task |
|---|---|
| § 3.1 `values.yaml` | Task 1 Step 4 |
| § 3.2 template (lines 97-99 unchanged, Go comment, conditional YAML comment, `default` and `toString`) | Task 1 Step 5 |
| § 3.3 `env.sh` header, `check_audience`, rows A1-A4, counter, bash 3.2 safety, mutation table | Task 1 Steps 1-3, 6, 7 |
| § 3.4 runbook (values table, § 5 row, § 6 item 1, Okta note and warning) | Task 3 Steps 1-4 |
| § 3.5 IAM Jail test | Task 2 |
| § 3.6 chart README | Task 3 Step 5 |
| § 3.7 not changed | Global Constraints; Task 4 Step 5 |
| § 4 AC 1 (byte-identical) | Task 1 Step 6; Task 4 Step 3 |
| § 4 AC 2 (A3 and the parse) | Task 1 row A3; Task 2 |
| § 4 AC 3 (runbook) | Task 3 |
| § 4 AC 4 (mutations) | Task 1 Step 7; Task 2 Step 3 |
| § 4 AC 5 (helm-render, IAM tests) | Task 4 Steps 1, 2, 4 |

Additions beyond the spec, from Review Focus: rows A5 and A6 (so `AUDIENCE_ROWS_WANT` is 6, not the spec's 4), the third argument of `check_audience` (the state of the comment line), mutations M6-M8, and the digit case plus mutation M-rs1 in Task 2.
