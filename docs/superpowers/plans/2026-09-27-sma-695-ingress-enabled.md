# SMA-695 `ingress.enabled` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the Helm value `ingress.enabled` (default `true`) to `charts/paigasus`. When it is `false`, the chart renders no Ingress and does not require `ingress.tlsSecretName`.

**Architecture:** One helper, `paigasus.ingressEnabled`, reads the value through `dig` and is the only reader. `templates/ingress.yaml` wraps its body in that helper. `paigasus.validate` gets a kind check for `ingress.enabled`, makes the `tlsSecretName` refusal conditional, and adds a host-form refusal. The default render stays byte-identical to the golden files.

**Tech Stack:** Helm 3.22.0 (pinned in `.prototools`), Go templates with Sprig, bash chart test scripts with inline Python 3 + PyYAML.

**Spec:** `docs/superpowers/specs/2026-09-27-sma-695-ingress-enabled-design.md`

## Global Constraints

- Work in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-695-ingress-enabled`, branch `feature/sma-695-helm-ingress-enabled`. Check `git branch --show-current` before the first edit.
- Prefix every shell with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so `helm` resolves to the pinned 3.22.0.
- Run chart scripts and the gate with `/bin/bash` (system bash 3.2). The scripts must also work under bash 5: no `mapfile`, no `declare -A`, no here-string (`<<<`), no pipe into `grep -q` (use `grep -qF -- X < <(printf '%s' "$out")`), and a `"${X[@]+"${X[@]}"}"` guard on every possibly-empty array.
- Every source file keeps its SPDX header.
- Do NOT add a new script under `charts/paigasus/tests/`. That raises `CHART_SCRIPT_FLOOR=7` in `ci/helm-render/run.sh` and `HELM_RENDER_SH_CALL_SITES` in `ci/affected-graph/ci_targets.py`.
- Do NOT run `charts/paigasus/tests/render.sh --update`. The golden files must not change. A golden change is a defect in this plan.
- A YAML comment inside a template renders into the manifest. Put no new YAML (`#`) comment in `templates/ingress.yaml`. Use `{{/* */}}` template comments in `.tpl` files.
- Conventional commits with a workspace scope, for example `feat(repo): …`. The commit body must not contain a line that starts with `#` followed by digits, or a `token: value` line (commitlint `footer-leading-blank`). End each commit message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Never use `git commit --amend`, `git reset`, `git stash`, or `--no-verify`. Never install host software (`brew install`). Do not start a background job.
- Write prose (docs, comments) in ASD-STE100 Simplified Technical English: short sentences, active voice, one idea per sentence.

## Review Focus

1. **A nil `ingress.enabled` with no chart default** (`--reuse-values` + `--set ingress.enabled=null` on a release from before this change). Expected: the Ingress stays. No test can reach this shape through `helm template` on the live chart, because the chart now has the default. The reviewer must read the `kindIs "invalid"` branch of the helper and the validator. The prototype measured it (spec § 4.2).
2. **The re-synced negative-control fixtures.** Expected: each fixture differs from its live file by exactly its old mutation. Task 1 Step 9 pins it with a `diff` before/after comparison.
3. **A GitOps values file with `enabled: "false"` (quoted).** Expected: a refusal that names `ingress.enabled`, not the `tlsSecretName` message. Row R3 pins it, and the kind check is placed before the `tlsSecretName` check.
4. **An operator who disables the Ingress and keeps `className` or `annotations`.** Expected: the render succeeds, and those values do nothing. Row R1 renders with the default `className: ""`. T1 renders with the defaults. No refusal exists for them.
5. **A host with uppercase letters or a trailing dot.** Expected: no refusal. Spec § 4.4 refuses only `/` and `:`. The API server still refuses uppercase when the Ingress is on. This is a known gap, not a task.

---

## File map

| File | Change | Task |
|---|---|---|
| `charts/paigasus/templates/_helpers.tpl` | new helper `paigasus.ingressEnabled`; three validation changes | 1 |
| `charts/paigasus/templates/ingress.yaml` | wrap the body in the helper | 1 |
| `charts/paigasus/values.yaml` | new key `ingress.enabled: true` (comments in Task 3) | 1 |
| `charts/paigasus/tests/ingress.sh` | rows T1 (two subsets), T2 | 1 |
| `charts/paigasus/tests/refusals.sh` | rows R1-R6 | 1 |
| `ci/helm-render/fixtures/literal-ingress/templates/ingress.yaml` | re-sync with the live file | 1 |
| `ci/helm-render/fixtures/slug-mirror/templates/_helpers.tpl` | re-sync | 1 |
| `ci/helm-render/fixtures/zones-omits-enabled/templates/_helpers.tpl` | re-sync | 1 |
| none (a record only) | mutation proof | 2 |
| `charts/paigasus/values.yaml` (comments), `charts/CLAUDE.md`, `charts/paigasus/README.md`, `docs/ops/RUNBOOK-chart.md` | docs | 3 |

---

### Task 1: `ingress.enabled` in the chart, with its tests and fixtures

**Files:**
- Modify: `charts/paigasus/templates/_helpers.tpl` (after `paigasus.name`, about line 41; the `tlsSecretName` check at lines 63-65; the `host` check at lines 121-123)
- Modify: `charts/paigasus/templates/ingress.yaml` (line 4 and the end of the file)
- Modify: `charts/paigasus/values.yaml:101-102`
- Test: `charts/paigasus/tests/ingress.sh`, `charts/paigasus/tests/refusals.sh`
- Modify: the three fixture files listed in the file map

**Interfaces:**
- Produces: the template `paigasus.ingressEnabled`. Call it as `include "paigasus.ingressEnabled" .`. It yields the string `true` (Ingress on) or `""` (Ingress off). Task 3's docs name it.
- Produces: these refusal message substrings, which the tests use as needles: `ingress.enabled must be true or false`, `ingress.tlsSecretName is required`, `ingress.host is required`, `must be a bare host name`.

- [ ] **Step 1: Save the fixture baseline**

Each fixture is a whole-file copy of a live file with one mutation. Save the current difference, so Step 9 can prove the mutation is unchanged.

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-695-ingress-enabled
B=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/f5c5949b-e745-4de0-8db0-da6d2dc623a1/scratchpad/fixture-baseline
mkdir -p "$B"
diff charts/paigasus/templates/ingress.yaml ci/helm-render/fixtures/literal-ingress/templates/ingress.yaml > "$B/literal-ingress.diff"
diff charts/paigasus/templates/_helpers.tpl ci/helm-render/fixtures/slug-mirror/templates/_helpers.tpl > "$B/slug-mirror.diff"
diff charts/paigasus/templates/_helpers.tpl ci/helm-render/fixtures/zones-omits-enabled/templates/_helpers.tpl > "$B/zones-omits-enabled.diff"
wc -l "$B"/*.diff
```

Expected: three non-empty files (`diff` exits 1 when the files differ; that is correct).

- [ ] **Step 2: Write the failing ingress tests (T1, T2)**

In `charts/paigasus/tests/ingress.sh`, insert this block after the two `coupling …` lines and before `if [ "$ec" -eq 0 ]; then echo "== chart ingress coupling OK =="; fi`:

```bash
# SMA-695. ingress.enabled=false renders no Ingress. Every enabled zone keeps its console Service
# (port name http, port 3000), because the operator's own route targets it. PAIGASUS_ZONES and
# PAIGASUS_SERVICES must still hold exactly the enabled zones. A separate function from
# `coupling`, which compares Ingress paths with the zone map and so needs an Ingress.
no_ingress() {
  local label="$1" want="$2"; shift 2
  local out got
  if ! out="$(helm template t "$CHART" "${BASE[@]+"${BASE[@]}"}" --set ingress.enabled=false "$@" 2>&1)"; then
    echo "FAIL [$label]: expected a successful render"; printf '%s\n' "$out"; ec=1; return
  fi
  got="$(printf '%s' "$out" | python3 -c '
import sys,yaml,json
want=sorted(sys.argv[1].split(","))
docs=[d for d in yaml.safe_load_all(sys.stdin) if d]
problems=[]
if any(d["kind"]=="Ingress" for d in docs):
    problems.append("an Ingress is rendered")
cm=[d for d in docs if d["kind"]=="ConfigMap" and d["metadata"]["name"].endswith("-zonemap")][0]
zones=sorted(json.loads(cm["data"]["PAIGASUS_ZONES"]))
services=sorted(json.loads(cm["data"]["PAIGASUS_SERVICES"]))
if zones!=want or services!=want:
    problems.append("zones=%s services=%s, want %s" % (zones,services,want))
for z in want:
    svc=[d for d in docs if d["kind"]=="Service" and d["metadata"]["name"].endswith("-%s-console" % z)]
    ports=[p for s in svc for p in s["spec"]["ports"] if p.get("name")=="http" and p.get("port")==3000]
    if len(svc)!=1 or len(ports)!=1:
        problems.append("zone %s: want one console Service with port http/3000, got %d Service(s)" % (z,len(svc)))
print("; ".join(problems) if problems else "NO-INGRESS-OK")' "$want")"
  if [ "$got" = "NO-INGRESS-OK" ]; then
    echo "  ok [$label]: no Ingress; console Services for $want"
  else
    echo "FAIL [$label]: $got"; ec=1
  fi
}

# SMA-695. A release made before ingress.enabled existed has no such key under
# `helm upgrade --reuse-values`. `--set ingress.enabled=null` deletes the key (measured on helm
# 3.22.0), which gives that shape. The Ingress must stay: a plain .Values.ingress.enabled reads nil
# there and would delete the live Ingress.
ingress_count() {
  local label="$1" want="$2"; shift 2
  local out n
  if ! out="$(helm template t "$CHART" "${BASE[@]+"${BASE[@]}"}" "$@" 2>&1)"; then
    echo "FAIL [$label]: expected a successful render"; printf '%s\n' "$out"; ec=1; return
  fi
  n="$(printf '%s' "$out" | python3 -c '
import sys,yaml
print(sum(1 for d in yaml.safe_load_all(sys.stdin) if d and d["kind"]=="Ingress"))')"
  if [ "$n" = "$want" ]; then
    echo "  ok [$label]: $n Ingress"
  else
    echo "FAIL [$label]: $n Ingress, want $want"; ec=1
  fi
}

no_ingress "ingress disabled, iam only" "iam" --set zones.gateway.enabled=false
no_ingress "ingress disabled, iam and gateway" "iam,gateway" --set zones.gateway.enabled=true
ingress_count "ingress.enabled key absent" 1 --set ingress.enabled=null
```

- [ ] **Step 3: Write the failing refusal rows (R1-R6)**

In `charts/paigasus/tests/refusals.sh`, insert this block directly after the existing row `expect_fail "ingress.tlsSecretName empty" …` (its two lines end with `--set ingress.tlsSecretName=""`):

```bash
# SMA-695. With ingress.enabled=false the chart renders no Ingress, so it needs no TLS Secret. The
# host stays required: it feeds PAIGASUS_PUBLIC_ORIGIN (spec D1).
expect_render "ingress disabled, tlsSecretName empty" \
  --set ingress.enabled=false --set ingress.tlsSecretName=""
expect_fail "ingress disabled, host empty" "ingress.host is required" \
  --set ingress.enabled=false --set ingress.host=""
# A quoted "false" is a string, and a non-empty string is true in a template `if`. Without this
# refusal the Ingress stays, which is the SMA-695 bug.
expect_fail "ingress.enabled a string" "ingress.enabled must be true or false" \
  --set-string ingress.enabled=false
# --set ingress.enabled=null deletes the key. The TLS refusal must then still fire, which catches a
# validator that reads .Values.ingress.enabled without paigasus.ingressEnabled.
expect_fail "ingress.enabled key absent, tlsSecretName empty" "ingress.tlsSecretName is required" \
  --set ingress.enabled=null --set ingress.tlsSecretName=""
# With the Ingress on, the API server refuses such a host. With it off, nothing else does, and
# https://https://… passes the console's https-URL parse.
expect_fail "ingress.host with a scheme" "must be a bare host name" \
  --set ingress.host=https://console.example.test
expect_fail "ingress.host with a port, ingress disabled" "must be a bare host name" \
  --set ingress.enabled=false --set ingress.host=console.example.test:8443
```

- [ ] **Step 4: Run the tests and see them fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash charts/paigasus/tests/ingress.sh; echo "rc=$?"
/bin/bash charts/paigasus/tests/refusals.sh --set ingress.host=console.example.test; echo "rc=$?"
```

Expected, `ingress.sh`: rc=1. Both `no_ingress` rows print `FAIL […]: an Ingress is rendered`. `ingress.enabled key absent` prints `ok` (1 Ingress) today. That is correct, because the old chart always renders the Ingress. It turns into a guard in Step 7.

Expected, `refusals.sh`: rc=1. These rows fail: `ingress disabled, tlsSecretName empty` (refused), `ingress.enabled a string` (rendered), `ingress.host with a scheme` and `ingress.host with a port, ingress disabled` (rendered). `ingress disabled, host empty` and `ingress.enabled key absent, tlsSecretName empty` print `ok` today, because the old refusals fire.

- [ ] **Step 5: Add the value and the helper**

In `charts/paigasus/values.yaml`, add the key as the first line of the `ingress` block (Task 3 adds the comments):

```yaml
ingress:
  enabled: true
  host: ""               # REQUIRED. Feeds PAIGASUS_PUBLIC_ORIGIN as https://<host>.
```

In `charts/paigasus/templates/_helpers.tpl`, insert this block directly before `{{- define "paigasus.enabledZones" -}}`:

```
{{/*
paigasus.ingressEnabled (SMA-695) yields "true" or "". Read through `dig` with the default true: a
release made before ingress.enabled existed has no such key under `helm upgrade --reuse-values`,
and a plain .Values.ingress.enabled would read nil and delete the live Ingress with no warning. A
nil value also counts as "not set": Helm keeps a null when no chart default shadows it.
paigasus.validate refuses a non-boolean value, so a quoted "false" never reaches this `if`.
*/}}
{{- define "paigasus.ingressEnabled" -}}
{{- $v := dig "enabled" true .Values.ingress -}}
{{- if or $v (kindIs "invalid" $v) -}}true{{- end -}}
{{- end -}}

```

- [ ] **Step 6: Change the validation**

In `paigasus.validate` in `_helpers.tpl`, replace these three lines:

```
{{- if not .Values.ingress.tlsSecretName -}}
{{- fail "ingress.tlsSecretName is required; PAIGASUS_PUBLIC_ORIGIN is validated as https and __Host-pgs_sid requires Secure, so the ingress must terminate TLS" -}}
{{- end -}}
```

with:

```
{{- $ingressEnabled := dig "enabled" true .Values.ingress -}}
{{- if not (or (kindIs "bool" $ingressEnabled) (kindIs "invalid" $ingressEnabled)) -}}
{{- fail (printf "ingress.enabled must be true or false (a boolean), got %s %q; a quoted \"false\" is a string and would keep the Ingress" (kindOf $ingressEnabled) (toString $ingressEnabled)) -}}
{{- end -}}
{{- if and (include "paigasus.ingressEnabled" .) (not .Values.ingress.tlsSecretName) -}}
{{- fail "ingress.tlsSecretName is required; PAIGASUS_PUBLIC_ORIGIN is validated as https and __Host-pgs_sid requires Secure, so the ingress must terminate TLS. Or set ingress.enabled: false when TLS ends in front of the chart's Services" -}}
{{- end -}}
```

Then replace these three lines:

```
{{- if not .Values.ingress.host -}}
{{- fail "ingress.host is required; it is the single origin every zone's cookie is scoped to" -}}
{{- end -}}
```

with:

```
{{- if not .Values.ingress.host -}}
{{- fail "ingress.host is required; it is the single origin every zone's cookie is scoped to, and it feeds PAIGASUS_PUBLIC_ORIGIN, so it is required also when ingress.enabled is false" -}}
{{- end -}}
{{- $host := .Values.ingress.host | toString -}}
{{- if or (contains "/" $host) (contains ":" $host) -}}
{{- fail (printf "ingress.host must be a bare host name with no scheme, path or port, got %q; PAIGASUS_PUBLIC_ORIGIN is https://<ingress.host>" $host) -}}
{{- end -}}
```

- [ ] **Step 7: Wrap the Ingress body**

In `charts/paigasus/templates/ingress.yaml`, insert one line directly after `{{- $full := include "paigasus.fullname" . -}}` (line 4), before `apiVersion: networking.k8s.io/v1`:

```
{{- if (include "paigasus.ingressEnabled" .) }}
```

Append one line at the very end of the file, after the last `          {{- end }}`:

```
{{- end }}
```

The file must still end with one newline. The `include "paigasus.validate"` line stays OUTSIDE the `if`.

- [ ] **Step 8: Run all chart tests and see them pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
for s in render ingress refusals maps names env ca-bundle; do
  if [ "$s" = refusals ]; then /bin/bash charts/paigasus/tests/$s.sh --set ingress.host=console.example.test
  else /bin/bash charts/paigasus/tests/$s.sh; fi
  echo "$s rc=$?"
done
git status --short charts/paigasus/tests/golden
```

Expected: every script prints its `== … OK ==` line and rc=0. `render.sh` prints `ok [iam-only]` and `ok [iam-and-gateway]`: the goldens are byte-identical. `git status` prints nothing for `tests/golden`.

- [ ] **Step 9: Re-sync the three fixtures**

Apply the identical edits of Steps 6 and 7 to the fixture copies. Do not change the lines of each fixture's mutation.

- `ci/helm-render/fixtures/literal-ingress/templates/ingress.yaml`: the Step 7 edits (the `if` line after the `$full` line, and `{{- end }}` at the end).
- `ci/helm-render/fixtures/slug-mirror/templates/_helpers.tpl` and `ci/helm-render/fixtures/zones-omits-enabled/templates/_helpers.tpl`: the Step 5 helper block and both Step 6 replacements.

Then prove that each fixture still differs from its live file by exactly its old mutation:

```bash
B=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/f5c5949b-e745-4de0-8db0-da6d2dc623a1/scratchpad/fixture-baseline
diff charts/paigasus/templates/ingress.yaml ci/helm-render/fixtures/literal-ingress/templates/ingress.yaml > "$B/literal-ingress.new"
diff charts/paigasus/templates/_helpers.tpl ci/helm-render/fixtures/slug-mirror/templates/_helpers.tpl > "$B/slug-mirror.new"
diff charts/paigasus/templates/_helpers.tpl ci/helm-render/fixtures/zones-omits-enabled/templates/_helpers.tpl > "$B/zones-omits-enabled.new"
for f in literal-ingress slug-mirror zones-omits-enabled; do
  sed 's/^[0-9,]*[acd][0-9,]*$/@@/' "$B/$f.diff" > "$B/$f.a"
  sed 's/^[0-9,]*[acd][0-9,]*$/@@/' "$B/$f.new" > "$B/$f.b"
  if cmp -s "$B/$f.a" "$B/$f.b"; then echo "$f: mutation unchanged"; else echo "$f: DIFFERENT"; diff "$B/$f.a" "$B/$f.b"; fi
done
```

Expected: three lines `…: mutation unchanged`. (The `sed` removes the line-number headers, which move because the live file grew.)

- [ ] **Step 10: Run the helm-render gate and its negative control**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash ci/helm-render/run.sh; echo "gate rc=$?"
/bin/bash ci/helm-render/run.sh --negative-control; echo "negative-control rc=$?"
/bin/bash ci/helm-render/run.sh --self-test; echo "self-test rc=$?"
```

Expected: rc=0 for all three. The negative control prints `OK` for each of the seven fixtures, and none prints `FAILED` or `INCONCLUSIVE`. If a fixture reports INCONCLUSIVE, the re-sync in Step 9 is wrong: read the fixture's render error before you change anything else.

- [ ] **Step 11: Commit**

```bash
git add charts/paigasus/templates/_helpers.tpl charts/paigasus/templates/ingress.yaml \
  charts/paigasus/values.yaml charts/paigasus/tests/ingress.sh charts/paigasus/tests/refusals.sh \
  ci/helm-render/fixtures/literal-ingress/templates/ingress.yaml \
  ci/helm-render/fixtures/slug-mirror/templates/_helpers.tpl \
  ci/helm-render/fixtures/zones-omits-enabled/templates/_helpers.tpl
git commit -m "feat(repo): allow the Helm chart Ingress to be disabled with ingress.enabled (SMA-695)

ingress.enabled defaults to true, and the default render is byte-identical.
When it is false, the chart renders no Ingress and does not require
ingress.tlsSecretName. ingress.host stays required, because it feeds
PAIGASUS_PUBLIC_ORIGIN. A non-boolean ingress.enabled and a host with a
scheme, path or port are refused.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git log --oneline -1
```

---

### Task 2: Mutation proof (spec § 5.1)

The test rows of Task 1 must fail when the feature is removed, not only before it exists. This task changes no committed file. It produces a record for the PR description.

**Files:**
- Temporary edits only: `charts/paigasus/templates/_helpers.tpl`, `charts/paigasus/templates/ingress.yaml`
- Create: `/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/f5c5949b-e745-4de0-8db0-da6d2dc623a1/scratchpad/sma-695-mutations.md`

**Interfaces:**
- Consumes: Task 1's commit. The working tree must be clean before this task starts (`git status --short` prints nothing).

- [ ] **Step 1: Record the protocol**

For each mutation in the table below:

1. Apply the mutation with the Edit tool. Change only the lines named.
2. Run the rows and `render.sh`:
   ```bash
   export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
   /bin/bash charts/paigasus/tests/ingress.sh > "$TMPDIR/m-ingress.log" 2>&1; echo "ingress rc=$?"
   grep -E '^FAIL|OK ==' "$TMPDIR/m-ingress.log"
   /bin/bash charts/paigasus/tests/refusals.sh --set ingress.host=console.example.test > "$TMPDIR/m-refusals.log" 2>&1; echo "refusals rc=$?"
   grep -E '^FAIL|OK ==' "$TMPDIR/m-refusals.log"
   /bin/bash charts/paigasus/tests/render.sh 2>&1 | tail -1
   ```
   The Bash tool runs zsh, so capture each rc before a pipe. Do not use `PIPESTATUS`. `$TMPDIR`
   may be unset: if so, use the scratchpad directory
   `/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/f5c5949b-e745-4de0-8db0-da6d2dc623a1/scratchpad`.
3. The mutation counts as proof only if its named row prints `FAIL` AND `render.sh` prints `== chart render OK ==`. A mutation that breaks the template parse reds every row for the wrong reason. Such a result is not proof: fix the mutation and run it again.
4. Restore the file with `git checkout -- <file>`. This is safe here only because Task 1 is committed and the tree was clean. Then check that `git status --short` prints nothing.

| # | Mutation | Row that must print FAIL |
|---|---|---|
| M1 | `ingress.yaml`: delete the `{{- if (include "paigasus.ingressEnabled" .) }}` line AND the last `{{- end }}` line | `ingress disabled, iam only` and `ingress disabled, iam and gateway` |
| M2 | helper: replace its two body lines with `{{- if .Values.ingress.enabled -}}true{{- end -}}` | `ingress.enabled key absent` |
| M3 | helper: replace its two body lines with `{{- if (default true .Values.ingress.enabled) -}}true{{- end -}}` | `ingress disabled, iam only` (an explicit false becomes true) |
| M4 | validate: change `{{- if and (include "paigasus.ingressEnabled" .) (not .Values.ingress.tlsSecretName) -}}` to `{{- if not .Values.ingress.tlsSecretName -}}` | `ingress disabled, tlsSecretName empty` |
| M5 | validate: change `{{- if not .Values.ingress.host -}}` to `{{- if and (include "paigasus.ingressEnabled" .) (not .Values.ingress.host) -}}` | `ingress disabled, host empty` |
| M6 | validate: delete the kind check (the `$ingressEnabled :=` line and the three-line `if … fail … end` after it) | `ingress.enabled a string` |
| M7 | validate: change `(include "paigasus.ingressEnabled" .)` in the tlsSecretName check to `.Values.ingress.enabled` | `ingress.enabled key absent, tlsSecretName empty` |
| M8 | validate: delete the three-line host-form `if … fail … end` (keep the `$host :=` line) | `ingress.host with a scheme` and `ingress.host with a port, ingress disabled` |

Note for M3: Sprig `default` treats `false` as empty and returns the default `true`. So under M3, `--set ingress.enabled=false` renders the Ingress, and T1 must print FAIL.

- [ ] **Step 2: Run M1 to M8 and write the record**

Write `/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/f5c5949b-e745-4de0-8db0-da6d2dc623a1/scratchpad/sma-695-mutations.md` with one table row per mutation: the mutation, the rows that printed FAIL (copied from the output), the `render.sh` result, and "proof" or "NOT proof". If a mutation is "NOT proof", stop and report it. Do not change a test to make it red.

- [ ] **Step 3: Confirm the tree is clean**

```bash
git status --short
git log --oneline -1
```

Expected: no output from `git status`. The last commit is still Task 1's commit.

---

### Task 3: Docs — the routing contract, the cut-over procedure, and the D6 limit

**Files:**
- Modify: `charts/paigasus/values.yaml` (header lines 3-6; the `ingress` block)
- Modify: `charts/CLAUDE.md` (the bullet "One values block renders three projections that must agree", lines 9-12)
- Modify: `charts/paigasus/README.md` ("One values block, six projections"; "The refusals"; a new section)
- Modify: `docs/ops/RUNBOOK-chart.md` (§ 1 table; § 2 list; § 4; a new section)

**Interfaces:**
- Consumes: the helper name `paigasus.ingressEnabled` and the refusal messages of Task 1.

- [ ] **Step 1: `values.yaml` comments**

After the header sentence that ends `… rather than merely checked (spec D6).` (line 6), add:

```yaml
# Exception (SMA-695, spec D2): with ingress.enabled false the chart renders no ingress path rules,
# and the operator's own route is a projection of `zones` that the operator keeps by hand.
```

Replace the `ingress` block's first lines (from `ingress:` through the two `tlsSecretName` comment lines) with:

```yaml
ingress:
  enabled: true          # false: the chart renders no Ingress. Route https://<host><basePath> to
                         # each enabled zone's console Service yourself: port name http, no
                         # rewrite, Host forwarded unchanged, TLS ended in front of the Services.
                         # See docs/ops/RUNBOOK-chart.md § 10. A boolean: a quoted "false" is refused.
  host: ""               # REQUIRED, also when enabled is false. Feeds PAIGASUS_PUBLIC_ORIGIN as
                         # https://<host>. A bare host name: no scheme, no path, no port.
  className: ""
  tlsSecretName: ""      # REQUIRED when enabled is true. The ingress must terminate TLS:
                         # PAIGASUS_PUBLIC_ORIGIN is validated as https, and __Host-pgs_sid
                         # requires Secure.
```

Keep `annotations: {}` and the two "DO NOT add a rewrite annotation" comment lines after it unchanged. `values.yaml` comments do not render into manifests, so the goldens do not change. Check this with `/bin/bash charts/paigasus/tests/render.sh`.

- [ ] **Step 2: `charts/CLAUDE.md`**

Replace the bullet that starts `- **One values block renders three projections that must agree:**` with:

```markdown
- **One values block renders three projections that must agree:** `PAIGASUS_ZONES`,
  `PAIGASUS_SERVICES` and the ingress rules. All three come from `zones.<id>.enabled` through one
  `range`. Do not write a zone literally in a template: `repo:helm-render` check 1 and check 2
  fail on it. With `ingress.enabled: false` (SMA-695) the chart renders no ingress rules, so only
  two projections remain, and the operator keeps the route by hand.
- **Read `ingress.enabled` only through `paigasus.ingressEnabled`.** It reads the value with `dig`
  and a default of `true`, and counts nil as not set. A plain `.Values.ingress.enabled` reads nil
  under `--reuse-values` on a release from before the value, and deletes the live Ingress.
```

- [ ] **Step 3: `charts/paigasus/README.md`**

a. In "One values block, six projections", after the sentence that ends `… so a zone cannot be routable but unadvertised or advertised but unrouted:`, the list stays. After the paragraph `A disabled zone leaves no trace …`, add:

```markdown
**Exception: `ingress.enabled: false` (SMA-695).** The chart then renders no Ingress, so the first
projection is gone. The operator's own route is a projection of `zones` that the operator keeps
by hand. A route to a disabled zone points to a deleted Service. An enabled zone with no route
gives a 404 for its `PAIGASUS_ZONES` link. SMA-694 (a chart-owned HTTPRoute) will restore D6 for
Gateway API.
```

b. In "The refusals", replace the bullet that starts `- **Every value \`values.yaml\` marks REQUIRED, when it is empty.**` so that its first sentence reads:

```markdown
- **Every value `values.yaml` marks REQUIRED, when it is empty.** `ingress.host`,
  `ingress.tlsSecretName` (only when `ingress.enabled` is true), `oidc.issuer`, `oidc.clientId`,
  `oidc.existingSecret`, `postgres.existingSecret` and `zones.iam.backend.apiKeysPepperSecret` each
  fail with their own named message the moment they are unset.
```

Keep the rest of that bullet unchanged. After the bullet, add:

```markdown
- **A non-boolean `ingress.enabled`, and an `ingress.host` with a scheme, path or port
  (SMA-695).** A quoted `"false"` is a string, and a string is true in a template `if`, so the
  Ingress would stay. A host such as `https://console.example.com` renders
  `PAIGASUS_PUBLIC_ORIGIN=https://https://console.example.com`. With the Ingress on, the API server
  refuses such a host. With it off, only this refusal does.
```

c. Add a new section after "No rewrite annotation":

```markdown
## Running without an Ingress controller

Set `ingress.enabled: false` when the cluster has no Ingress controller, for example a cluster
that uses Gateway API. The chart then renders no Ingress, and `ingress.tlsSecretName` is not
required. `ingress.host` stays required, because it feeds `PAIGASUS_PUBLIC_ORIGIN` and the OIDC
redirect URIs. `ingress.className` and `ingress.annotations` then do nothing.

`templates/ingress.yaml` wraps its body in `paigasus.ingressEnabled` (`templates/_helpers.tpl`).
`tests/ingress.sh` and `tests/refusals.sh` hold the SMA-695 rows. The routing contract that the
operator must meet is in `docs/ops/RUNBOOK-chart.md` § 10.
```

- [ ] **Step 4: `docs/ops/RUNBOOK-chart.md`**

a. § 1 table. Insert a new row directly before the `ingress.host` row:

```markdown
| `ingress.enabled` | no | Default `true`. `false`: the chart renders no Ingress, and you route the traffic yourself (§ 10). It must be a boolean |
```

Replace the `ingress.host` row and the `ingress.tlsSecretName` row with:

```markdown
| `ingress.host` | yes, also when `ingress.enabled` is false | The one public host. `PAIGASUS_PUBLIC_ORIGIN` is `https://<host>`. A bare host name: no scheme, no path, no port |
```

```markdown
| `ingress.tlsSecretName` | when `ingress.enabled` is true | The TLS Secret for `ingress.host`. The ingress must end TLS |
```

b. § 2 list. After the item `- an empty value for each required key in § 1;`, add:

```markdown
- `ingress.enabled` set to a value that is not a boolean (a quoted `"false"` is a string);
- `ingress.host` with a scheme, a path or a port;
```

c. § 4. After the paragraph that starts `**The limit of D6.**`, add:

```markdown
**D6 and `ingress.enabled: false` (SMA-695).** With the Ingress off, the chart renders no routing.
Your own route is then a seventh projection, and you keep it by hand. Change the route in the same
change as any `zones.<id>.enabled` edit. A route to a disabled zone points to a deleted Service. An
enabled zone with no route gives a 404 for its link.
```

d. Add a new section at the end of the file. First read the last section number in the file with `grep -n '^## ' docs/ops/RUNBOOK-chart.md`. If the last section is not § 9, use the next free number and change the `§ 10` references in Steps 1, 3 and 4a to that number.

```markdown
## 10. Running without an Ingress controller (`ingress.enabled: false`)

Use this when the cluster has no Ingress controller, for example a K3s cluster with Cilium
Gateway API. Without a controller, the Ingress never gets a load-balancer status, and Argo CD
reports the Application as Progressing forever.

**The routing contract.** Your route must:

1. Send each enabled zone's `basePath` prefix on `ingress.host` to that zone's console Service,
   port name `http` (port 3000). Take the Service name from the rendered manifest
   (`helm template` or `kubectl get svc`). The chart shortens the base of a long release name, so
   `<release>-paigasus-<zone>-console` is not always the name.
2. Not rewrite the path. Each console serves its full `basePath` (§ 3).
3. Forward the `Host` header (or `X-Forwarded-Host`) unchanged. Next's Server Action origin check
   fails otherwise. An HTTPRoute `URLRewrite` hostname filter breaks it.
4. End TLS for `ingress.host` at the Gateway or proxy in front of the chart's Services.
5. Send traffic only to the console Services. Never expose a `*-backend` Service.
6. Change together with `zones.<id>.enabled` (§ 4).

**Cut-over on a release that has a live Ingress.**

1. Create the new route and verify it.
2. Set `ingress.enabled: false` and sync.
3. Remove the old Ingress. `helm upgrade` deletes it. Argo CD marks it "requires pruning" and
   keeps it, unless the sync prunes. Sync with prune, or delete the Ingress by hand. A kept Ingress
   can keep the Application Progressing.

SMA-694 will add a chart-owned Gateway API HTTPRoute.
```

- [ ] **Step 5: Check the docs and the goldens**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash charts/paigasus/tests/render.sh | tail -1
grep -n "§ 10\|§ 1[0-9]" charts/paigasus/values.yaml charts/paigasus/README.md docs/ops/RUNBOOK-chart.md
grep -n '^## ' docs/ops/RUNBOOK-chart.md
```

Expected: `== chart render OK ==`. Every `§ N` reference to the new section names the number of the section that Step 4d added.

- [ ] **Step 6: Commit**

```bash
git add charts/paigasus/values.yaml charts/CLAUDE.md charts/paigasus/README.md docs/ops/RUNBOOK-chart.md
git commit -m "docs(repo): document running the Helm chart without an Ingress controller (SMA-695)

Adds the routing contract and the Argo CD cut-over procedure for
ingress.enabled false, and states where decision D6 stops holding.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git log --oneline -1
```

---

## Final verification (the controller runs this after Task 3)

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash ci/helm-render/run.sh; echo "gate rc=$?"
/bin/bash ci/helm-render/run.sh --negative-control; echo "negative-control rc=$?"
git diff --stat origin/main...HEAD
```

Then the affected graph for this branch, as CI runs it (`CLAUDE.md`, "Before you push"). On this Mac, `repo:affected-smoke` needs system bash 3.2 and `repo:actionlint` needs bash 5 and a healthy pipe. Read each gate's own result as `CLAUDE.md` § "This development Mac only" says.
