<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-694 Helm chart HTTPRoute Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** With `httpRoute.enabled: true`, `charts/paigasus` renders one `gateway.networking.k8s.io/v1` HTTPRoute for each enabled zone, from the same `range` over `zones` as the Ingress, with fail-closed validation, optional per-rule timeouts and optional annotations.

**Architecture:** A new helper file `templates/_httproute.tpl` holds the route helpers and `paigasus.validateHttpRoute`. `paigasus.validate` calls it with one new line. A new template `templates/httproute.yaml` renders the routes inside `if (include "paigasus.httpRouteEnabled" .)`. The route checks go into the existing chart scripts (`tests/ingress.sh`, `tests/refusals.sh`, `tests/names.sh`, `tests/render.sh`), not into `helm_render.py`.

**Tech Stack:** Helm 3.22.0 (Go templates + sprig), bash (3.2 and 5.x), python3 + PyYAML for the render assertions.

**Spec:** `docs/superpowers/specs/2026-09-27-sma-694-helm-httproute-design.md`. Read it with this plan. The section numbers below (D1-D14, § 4.4, § 5.1, § 5.2, § 5.4, § 6) refer to it.

## Global Constraints

- Work only in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-694-helm-httproute`, branch `feature/sma-694-helm-httproute`. Start every shell command with `cd <worktree> &&`.
- Prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. Run `helm` only with the worktree as the working directory. MEASURED on 2026-09-28: the proto shim resolves helm 3.22.0 inside the repo and helm **4.3.0** outside it, and the goldens then differ by blank lines. Check with `helm version --short` → `v3.22.0+g144ca65`.
- Every new source file opens with an SPDX header (`{{/* SPDX-License-Identifier: Apache-2.0 */}}` in a template, `# SPDX-License-Identifier: Apache-2.0` in YAML and shell, `<!-- SPDX-License-Identifier: Apache-2.0 -->` in Markdown).
- Conventional commits with the scope `repo` (as SMA-695). End each commit message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never use `--no-verify`, `--no-gpg-sign`, `git commit --amend` or `git reset`. If signing fails with "failed to fill whole buffer", stop: 1Password is locked.
- Chart scripts run under system `/bin/bash` (3.2.57) AND `/opt/homebrew/bin/bash` (5.x). Keep the `"${X[@]+"${X[@]}"}"` guards on arrays that can be empty. Never pipe into `grep -q`; use process substitution (actionlint check 13).
- The apiVersion is exactly `gateway.networking.k8s.io/v1`. The kind is exactly `HTTPRoute`.
- No comment in `templates/httproute.yaml` contains the word "gateway" in any case (§ 4.3). Write "the route's parents", "the listener", "the chat relay".
- Every YAML comment in `httproute.yaml` stays inside the `if` block. A comment that names a path is written per zone.
- The two existing goldens `tests/golden/iam-only.yaml` and `tests/golden/iam-and-gateway.yaml` do not change (AC4). `git diff --stat` must never list them.
- No new REQUIRED value (D9). Do not edit `ci/helm-render/helm_render.py`, `ci/helm-render/run.sh`, `ci/affected-graph/ci_targets.py`, `ci/kind/`, `templates/ingress.yaml`, `templates/console-deployment.yaml` or `Chart.yaml` (§ 4.5).
- No `httpRoute.hostnames`, no `filters` value, no `.Capabilities` gate (D2, D7, D8).
- The timeout form is `^([0-9]{1,5}(h|m|s|ms)){1,4}$`. The default is `zones.gateway.console.httpRouteTimeouts.request: 10m`. The iam zone has no default.
- Do not install host software (no brew, no global npm). Do not start background jobs.

## Review Focus

1. **Millisecond durations in the order check.** `request: 900ms` with `backendRequest: 1s` must be refused, and `request: 1m` with `backendRequest: 59s999ms` must render. A parser that reads `ms` as `m` gets both wrong. Rows in Task 4 (`refusals.sh`).
2. **The zone default takes part in the order check.** `httpRoute.timeouts.backendRequest: 20m` with the gateway zone on must fail for `zones.gateway` (its merged `request` is the `10m` default), although no key the operator set is wrong. Row in Task 4 (`refusals.sh`).
3. **An empty zone key does not hide the chart key.** `httpRoute.timeouts.request: 30s` with `zones.gateway.console.httpRouteTimeouts.request: ""` gives the gateway rule `request: 30s`, not `10m` and not an empty value. Row in Task 4 (`ingress.sh`).
4. **The lowercase refusal is only for route mode.** With `httpRoute.enabled: false`, an `ingress.host` of `Console.Example.test` still renders, as before this change. Row in Task 1 (`refusals.sh`).
5. **A partial `httpRoute` map from `--reuse-values`.** With `httpRoute.enabled: true` and the `timeouts` and `annotations` keys absent (`--set httpRoute.timeouts=null --set httpRoute.annotations=null`), the chart renders, the gateway rule keeps `request: 10m`, and no route has `annotations`. Row in Task 4 (`ingress.sh`).

---

## File map

| File | Task | Responsibility |
|---|---|---|
| `charts/paigasus/templates/_httproute.tpl` (new) | 1, 2, 3, 4 | The route helpers and `paigasus.validateHttpRoute` |
| `charts/paigasus/templates/_helpers.tpl` | 1 | One include line in `paigasus.validate` |
| `ci/helm-render/fixtures/slug-mirror/templates/_helpers.tpl` | 1 | Re-sync of the one include line |
| `ci/helm-render/fixtures/zones-omits-enabled/templates/_helpers.tpl` | 1 | Re-sync of the one include line |
| `charts/paigasus/values.yaml` | 1, 3, 4, 6 | The `httpRoute` block, the gateway zone key, the header |
| `charts/paigasus/templates/httproute.yaml` (new) | 2, 3, 4 | The HTTPRoute objects |
| `charts/paigasus/templates/console-service.yaml` | 2 | The port from `paigasus.consoleServicePort` |
| `charts/paigasus/tests/refusals.sh` | 1, 3, 4 | Refusal rows R1-R16 and Review Focus 1, 2, 4 |
| `charts/paigasus/tests/ingress.sh` | 2, 3, 4 | Rows H1-H8 and Review Focus 3, 5 |
| `charts/paigasus/tests/names.sh` | 2 | Route flags on the four release rows |
| `charts/paigasus/tests/render.sh` + `tests/golden/iam-and-gateway-httproute.yaml` (new) | 5 | The third golden |
| `charts/CLAUDE.md`, `charts/paigasus/README.md`, `docs/ops/RUNBOOK-chart.md` | 6 | The docs of § 6 |
| this plan, section "Mutation record" | 7 | The manual mutation record (§ 5.2) for the PR body |

Shared shell prefix, written `$P` below (it is not a variable; paste it):

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-694-helm-httproute && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
```

`run.sh` passes `--set ingress.host=console.example.test` to each chart script. When you run a chart script directly, pass the same argument.

---

### Task 1: The `httpRoute` values block and its core validation

**Files:**
- Create: `charts/paigasus/templates/_httproute.tpl`
- Modify: `charts/paigasus/templates/_helpers.tpl:150` (add one line after it)
- Modify: `ci/helm-render/fixtures/slug-mirror/templates/_helpers.tpl`, `ci/helm-render/fixtures/zones-omits-enabled/templates/_helpers.tpl` (the same one line)
- Modify: `charts/paigasus/values.yaml` (new block after the `ingress` block, line 117)
- Test: `charts/paigasus/tests/refusals.sh`

**Interfaces:**
- Produces: `paigasus.httpRouteEnabled` (root context → `"true"` or `""`); `paigasus.validateHttpRoute` (root context → fails or yields `""`). Later tasks insert blocks into `paigasus.validateHttpRoute` after the lowercase check.
- Produces: the bash array `ROUTE` in `refusals.sh` (a valid route-mode flag set) that Tasks 3 and 4 reuse.

- [ ] **Step 1: Write the failing refusal rows**

In `charts/paigasus/tests/refusals.sh`, insert this block directly before the line `expect_render "iam only" --set zones.gateway.enabled=false` (near the end of the file):

```bash
# SMA-694 (spec § 4.4). The HTTPRoute block. ROUTE is a valid route-mode flag set, so a row
# below fails only for the value it names. Each needle carries its key path, so it cannot match
# another refusal.
ROUTE=(--set httpRoute.enabled=true --set 'httpRoute.parentRefs[0].name=gw'
  --set 'httpRoute.parentRefs[0].sectionName=https')
# R1. A quoted "false" is a string, and a non-empty string is true in a template `if`.
expect_fail "httpRoute.enabled a string" "httpRoute.enabled must be true or false" \
  --set-string httpRoute.enabled=false
# R2, R2b, R2c. An absent, null or non-list parentRefs attaches the route to nothing. R2b is the
# `helm upgrade --reuse-values` shape of a release made before the value (AC5). For R2c, helm
# 3.22.0 keeps the map over the default list with a coalesce warning (measured 2026-09-28).
expect_fail "httpRoute without parentRefs" "httpRoute.parentRefs is required" \
  --set httpRoute.enabled=true
expect_fail "httpRoute parentRefs null" "httpRoute.parentRefs is required" \
  --set httpRoute.enabled=true --set httpRoute.parentRefs=null
expect_fail "httpRoute parentRefs a map" "httpRoute.parentRefs is required" \
  --set httpRoute.enabled=true --set httpRoute.parentRefs.name=gw
# R3, R4. Each entry names a parent and selects one listener (D11).
expect_fail "httpRoute parentRef without name" "httpRoute.parentRefs[0] must be a map with a non-empty name" \
  --set httpRoute.enabled=true --set 'httpRoute.parentRefs[0].sectionName=https'
expect_fail "httpRoute parentRef without listener" "httpRoute.parentRefs[0] must set sectionName or port" \
  --set httpRoute.enabled=true --set 'httpRoute.parentRefs[0].name=gw'
# R5. `dig` needs a map.
expect_fail "httpRoute not a map" "httpRoute must be a map" \
  --set httpRoute=true
# R6. The API server refuses an HTTPRoute hostname with an upper-case letter.
expect_fail "httpRoute host not lowercase" "ingress.host must be lowercase when httpRoute.enabled" \
  "${ROUTE[@]}" --set ingress.host=Console.Example.test
# Review Focus 4. The lowercase rule is for route mode only. The Ingress path is unchanged.
expect_render "upper-case host, httpRoute off" \
  --set ingress.host=Console.Example.test
# R7. A port selects the listener too.
expect_render "httpRoute parentRef with port" \
  --set httpRoute.enabled=true --set 'httpRoute.parentRefs[0].name=gw' \
  --set 'httpRoute.parentRefs[0].port=443'
```

- [ ] **Step 2: Run the rows and see them fail**

Run: `$P && /bin/bash charts/paigasus/tests/refusals.sh --set ingress.host=console.example.test; echo rc=$?`
Expected: `rc=1`. R1, R2, R2b, R2c, R3, R4 and R6 print `FAIL [...]: rendered, expected a refusal`. R5 prints `FAIL [httpRoute not a map]: refused, but not with "httpRoute must be a map"` or a render. The two `expect_render` rows print `ok`.

- [ ] **Step 3: Create `templates/_httproute.tpl`**

```gotemplate
{{/* SPDX-License-Identifier: Apache-2.0 */}}

{{/*
The chart-owned HTTPRoute (SMA-694). Each helper in this file takes the ROOT context, unless its
comment says otherwise. Every caller reads the block as `.Values.httpRoute | default dict`: `dig`
needs a map, and under `helm upgrade --reuse-values` a release made before this block existed has
no httpRoute map. paigasus.validateHttpRoute refuses a httpRoute that is not a map before any other
helper reads it. This file has no negative-control fixture copy; a fixture chart gets it from the
live chart (ci/helm-render/run.sh copies the live chart, then overlays the fixture).
*/}}

{{/*
paigasus.httpRouteEnabled yields "true" or "". Nil counts as not set, which is false here.
paigasus.validateHttpRoute refuses a non-boolean value, so a quoted "false" never reaches an `if`.
Read httpRoute.enabled ONLY through this helper.
*/}}
{{- define "paigasus.httpRouteEnabled" -}}
{{- $h := .Values.httpRoute | default dict -}}
{{- if kindIs "map" $h -}}
{{- if dig "enabled" false $h -}}true{{- end -}}
{{- end -}}
{{- end -}}

{{/*
paigasus.validateHttpRoute: the refusals for the httpRoute block (spec § 4.4). paigasus.validate
calls it last, so the ingress.host refusals (required, bare host) have already run. An ABSENT key
counts as its empty value: under `helm upgrade --reuse-values` a release made before this change
has no parentRefs key, and the API server accepts an HTTPRoute with no parentRefs, which attaches
to nothing.
*/}}
{{- define "paigasus.validateHttpRoute" -}}
{{- $raw := .Values.httpRoute -}}
{{- if not (or (kindIs "invalid" $raw) (kindIs "map" $raw)) -}}
{{- fail (printf "httpRoute must be a map, got %s" (kindOf $raw)) -}}
{{- end -}}
{{- $h := $raw | default dict -}}
{{- $en := dig "enabled" false $h -}}
{{- if not (or (kindIs "bool" $en) (kindIs "invalid" $en)) -}}
{{- fail (printf "httpRoute.enabled must be true or false (a boolean), got %s %q; a quoted \"false\" is a string and would render the HTTPRoutes" (kindOf $en) (toString $en)) -}}
{{- end -}}
{{- if (include "paigasus.httpRouteEnabled" .) -}}
{{- $refs := dig "parentRefs" list $h -}}
{{- if not (and (kindIs "slice" $refs) $refs) -}}
{{- fail "httpRoute.parentRefs is required when httpRoute.enabled is true: a non-empty list that names the listener serving ingress.host" -}}
{{- end -}}
{{- range $i, $r := $refs -}}
{{- if not (and (kindIs "map" $r) (kindIs "string" $r.name) $r.name) -}}
{{- fail (printf "httpRoute.parentRefs[%d] must be a map with a non-empty name" $i) -}}
{{- end -}}
{{- if not (or $r.sectionName $r.port) -}}
{{- fail (printf "httpRoute.parentRefs[%d] must set sectionName or port to select the HTTPS listener; without one the route attaches to every listener, a plain HTTP one included" $i) -}}
{{- end -}}
{{- end -}}
{{- $host := .Values.ingress.host | toString -}}
{{- if ne $host (lower $host) -}}
{{- fail (printf "ingress.host must be lowercase when httpRoute.enabled is true (Gateway API hostnames are lowercase RFC 1123 names), got %q" $host) -}}
{{- end -}}
{{- end -}}
{{- end -}}
```

- [ ] **Step 4: Call the validator from `paigasus.validate`**

In `charts/paigasus/templates/_helpers.tpl`, replace

```gotemplate
{{- include "paigasus.validateIamBackend" . -}}
{{- end -}}
```

with

```gotemplate
{{- include "paigasus.validateIamBackend" . -}}
{{- include "paigasus.validateHttpRoute" . -}}
{{- end -}}
```

Make the same replacement in `ci/helm-render/fixtures/slug-mirror/templates/_helpers.tpl` and in `ci/helm-render/fixtures/zones-omits-enabled/templates/_helpers.tpl` (the `charts/CLAUDE.md` fixture rule). Keep each fixture's mutation. Check:

Run: `$P && for f in slug-mirror zones-omits-enabled; do diff charts/paigasus/templates/_helpers.tpl ci/helm-render/fixtures/$f/templates/_helpers.tpl; done`
Expected: only the two known mutations: `slug-mirror` line 9 (`iam gateway billing`) and `zones-omits-enabled` lines 156-157 (the `NEGATIVE-CONTROL FIXTURE` comment and `(ne $id "gateway")`). No line about `validateHttpRoute`.

- [ ] **Step 5: Add the values block**

In `charts/paigasus/values.yaml`, replace

```yaml
  # DO NOT add a rewrite annotation. Each console compiles basePath in and serves its full path
  # already; a rewrite that strips /iam breaks every route in that zone.

oidc:
```

with

```yaml
  # DO NOT add a rewrite annotation. Each console compiles basePath in and serves its full path
  # already; a rewrite that strips /iam breaks every route in that zone.

httpRoute:
  # NOT required. true renders one gateway.networking.k8s.io/v1 HTTPRoute for each enabled zone,
  # from the same zones range as the Ingress (SMA-694). It must be a boolean: a quoted "false" is
  # refused. Independent of ingress.enabled: both can be on during a cut-over. See
  # docs/ops/RUNBOOK-chart.md § 11. The route host is always ingress.host, and ingress.host must
  # then be lowercase.
  enabled: false
  # REQUIRED when enabled is true. Gateway API ParentReference objects, rendered as written.
  # Each entry needs a name and a sectionName or port that selects the HTTPS listener: an entry
  # with neither attaches to every listener, a plain HTTP one included, and is refused.
  # Example: [{name: cilium-gateway, namespace: kube-system, sectionName: https}]
  # The listener must end TLS for ingress.host and allow this release's namespace.
  parentRefs: []
  # DO NOT add a rewrite or any other filter. Each console serves its full basePath.

oidc:
```

- [ ] **Step 6: Run the rows and see them pass; check the goldens**

Run: `$P && /bin/bash charts/paigasus/tests/refusals.sh --set ingress.host=console.example.test; echo rc=$?; /opt/homebrew/bin/bash charts/paigasus/tests/refusals.sh --set ingress.host=console.example.test | tail -1; /bin/bash charts/paigasus/tests/render.sh --set ingress.host=console.example.test | tail -1`
Expected: `rc=0`, `== chart refusals OK ==` twice, and `== chart render OK ==`.

- [ ] **Step 7: Run the negative control**

Run: `$P && /bin/bash ci/helm-render/run.sh --negative-control; echo rc=$?`
Expected: `rc=0`. Each fixture prints `negative-control OK [<name>]`. No `INCONCLUSIVE`.

- [ ] **Step 8: Commit**

```bash
$P && git branch --show-current   # must print feature/sma-694-helm-httproute
git add charts/paigasus/templates/_httproute.tpl charts/paigasus/templates/_helpers.tpl \
  ci/helm-render/fixtures/slug-mirror/templates/_helpers.tpl \
  ci/helm-render/fixtures/zones-omits-enabled/templates/_helpers.tpl \
  charts/paigasus/values.yaml charts/paigasus/tests/refusals.sh
git commit -m "feat(repo): validate the Helm chart httpRoute values block (SMA-694)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The HTTPRoute template, the port helper, and the coupling rows

**Files:**
- Modify: `charts/paigasus/templates/_httproute.tpl` (add `paigasus.consoleServicePort`)
- Create: `charts/paigasus/templates/httproute.yaml`
- Modify: `charts/paigasus/templates/console-service.yaml:18`
- Test: `charts/paigasus/tests/ingress.sh`, `charts/paigasus/tests/names.sh`

**Interfaces:**
- Consumes: `paigasus.httpRouteEnabled`, `paigasus.validate` (Task 1), `paigasus.name` (`_helpers.tpl`).
- Produces: `paigasus.consoleServicePort` (any context → `3000`). The template `httproute.yaml` with the local variables `$root`, `$h`, `$id`, `$z`, `$name`, that Tasks 3 and 4 extend. The bash array `ROUTE` and the function `route_shape` in `ingress.sh`, that Tasks 3 and 4 reuse.

- [ ] **Step 1: Write the failing coupling rows**

In `charts/paigasus/tests/ingress.sh`, insert this block directly before the line `if [ "$ec" -eq 0 ]; then echo "== chart ingress coupling OK =="; fi`:

```bash
# SMA-694. httpRoute.enabled=true renders one HTTPRoute per enabled zone, from the same zones range
# as the Ingress. ROUTE holds a valid parentRef, so the parentRefs refusal does not block a row.
ROUTE=(--set httpRoute.enabled=true --set 'httpRoute.parentRefs[0].name=gw'
  --set 'httpRoute.parentRefs[0].sectionName=https')

# H1-H3 (spec § 5.1). $want is the comma list of enabled zones. $absent is a word that must not
# appear in the render once the HTTPRoute apiVersion lines are removed (H2): every HTTPRoute
# renders `apiVersion: gateway.networking.k8s.io/v1`, so the raw render always holds "gateway".
# Do NOT weaken the needle to make H2 pass. Remove only the apiVersion line.
route_coupling() {
  local label="$1" want="$2" absent="$3"; shift 3
  local out got stripped
  if ! out="$(helm template t "$CHART" "${BASE[@]+"${BASE[@]}"}" "${ROUTE[@]}" "$@" 2>&1)"; then
    echo "FAIL [$label]: expected a successful render"; printf '%s\n' "$out"; ec=1; return
  fi
  got="$(printf '%s' "$out" | python3 -c '
import sys,yaml,json
want=sorted(sys.argv[1].split(","))
docs=[d for d in yaml.safe_load_all(sys.stdin) if d]
problems=[]
routes=[d for d in docs if d["kind"]=="HTTPRoute"]
cm=[d for d in docs if d["kind"]=="ConfigMap" and d["metadata"]["name"].endswith("-zonemap")][0]
zones=json.loads(cm["data"]["PAIGASUS_ZONES"])
if sorted(zones)!=want:
    problems.append("PAIGASUS_ZONES keys %s, want %s" % (sorted(zones),want))
if len(routes)!=len(want):
    problems.append("%d HTTPRoute(s), want %d" % (len(routes),len(want)))
paths=[]
for r in routes:
    n=r["metadata"]["name"]; spec=r["spec"]
    if r["apiVersion"]!="gateway.networking.k8s.io/v1":
        problems.append("%s: apiVersion %s" % (n,r["apiVersion"]))
    if spec.get("hostnames")!=["console.example.test"]:
        problems.append("%s: hostnames %s" % (n,spec.get("hostnames")))
    rules=spec.get("rules") or []
    if len(rules)!=1:
        problems.append("%s: %d rules, want 1" % (n,len(rules))); continue
    rule=rules[0]
    if "filters" in rule:
        problems.append("%s: the rule has filters" % n)
    m=rule.get("matches") or []
    if len(m)!=1 or m[0].get("path",{}).get("type")!="PathPrefix":
        problems.append("%s: want one PathPrefix match, got %s" % (n,m)); continue
    paths.append(m[0]["path"]["value"])
    refs=rule.get("backendRefs") or []
    if len(refs)!=1:
        problems.append("%s: %d backendRefs, want 1" % (n,len(refs))); continue
    svc=[d for d in docs if d["kind"]=="Service" and d["metadata"]["name"]==refs[0].get("name")]
    if len(svc)!=1:
        problems.append("%s: backendRef %s names no rendered Service" % (n,refs[0].get("name"))); continue
    if refs[0].get("port") not in [p.get("port") for p in svc[0]["spec"]["ports"]]:
        problems.append("%s: backendRef port %s is not a port of Service %s" % (n,refs[0].get("port"),svc[0]["metadata"]["name"]))
    sel=svc[0]["spec"]["selector"]
    deps=[d for d in docs if d["kind"]=="Deployment" and all(d["spec"]["template"]["metadata"]["labels"].get(k)==v for k,v in sel.items())]
    zone_env=[e.get("value") for d in deps for c in d["spec"]["template"]["spec"]["containers"] for e in c.get("env",[]) if e.get("name")=="PAIGASUS_ZONE"]
    if len(deps)!=1 or zones.get(zone_env[0] if zone_env else None)!=m[0]["path"]["value"]:
        problems.append("%s: path %s reaches the Deployment(s) of zone(s) %s" % (n,m[0]["path"]["value"],zone_env))
if sorted(paths)!=sorted(zones.values()):
    problems.append("route paths %s, want %s" % (sorted(paths),sorted(zones.values())))
print("; ".join(problems) if problems else "ROUTE-OK paths=%s" % ",".join(sorted(paths)))' "$want")"
  if [ "${got:0:8}" = "ROUTE-OK" ]; then
    echo "  ok [$label]: $got"
  else
    echo "FAIL [$label]: $got"; ec=1
  fi
  if [ -n "$absent" ]; then
    stripped="$(printf '%s\n' "$out" | grep -v '^apiVersion: gateway\.networking\.k8s\.io/' || true)"
    if grep -qiF -- "$absent" < <(printf '%s' "$stripped"); then
      echo "FAIL [$label]: disabled zone \"$absent\" appears in the rendered output"; ec=1
    fi
  fi
}

# H4-H8 (spec § 5.1). Prints, per HTTPRoute, "<zone>:<timeouts JSON or ->:<annotations JSON or ->",
# sorted, joined by one space, or "none". The zone comes from the selector of the Service behind
# the backendRef, so a swapped name cannot hide.
route_shape() {
  local label="$1" want="$2"; shift 2
  local out got
  if ! out="$(helm template t "$CHART" "${BASE[@]+"${BASE[@]}"}" "$@" 2>&1)"; then
    echo "FAIL [$label]: expected a successful render"; printf '%s\n' "$out"; ec=1; return
  fi
  got="$(printf '%s' "$out" | python3 -c '
import sys,yaml,json
docs=[d for d in yaml.safe_load_all(sys.stdin) if d]
rows=[]
for r in (d for d in docs if d["kind"]=="HTTPRoute"):
    rule=r["spec"]["rules"][0]
    svc=[d for d in docs if d["kind"]=="Service" and d["metadata"]["name"]==rule["backendRefs"][0]["name"]][0]
    zone=svc["spec"]["selector"]["app.kubernetes.io/name"].rsplit("-console",1)[0]
    t=json.dumps(rule["timeouts"],sort_keys=True,separators=(",",":")) if "timeouts" in rule else "-"
    a=json.dumps(r["metadata"]["annotations"],sort_keys=True,separators=(",",":")) if "annotations" in r["metadata"] else "-"
    rows.append("%s:%s:%s" % (zone,t,a))
print(" ".join(sorted(rows)) if rows else "none")')"
  if [ "$got" = "$want" ]; then
    echo "  ok [$label]: $got"
  else
    echo "FAIL [$label]: got \"$got\", want \"$want\""; ec=1
  fi
}

# H1, H2 (AC1-AC3).
route_coupling "route, iam only" "iam" "gateway" --set zones.gateway.enabled=false
route_coupling "route, iam and gateway" "iam,gateway" "" --set zones.gateway.enabled=true
# H3 (D3, AC7). Route mode with the Ingress off needs no TLS Secret.
route_coupling "route, ingress disabled, no TLS Secret" "iam,gateway" "" --set zones.gateway.enabled=true \
  --set ingress.enabled=false --set ingress.tlsSecretName=""
ingress_count "route, ingress disabled: no Ingress" 0 "${ROUTE[@]}" --set zones.gateway.enabled=true \
  --set ingress.enabled=false --set ingress.tlsSecretName=""
# H4 (AC5). A release made before this change has no httpRoute map.
route_shape "route, httpRoute key absent" "none" "${ROUTE[@]}" --set zones.gateway.enabled=true \
  --set httpRoute=null
```

Also in `charts/paigasus/tests/names.sh`, replace the four `check` lines (lines 72-75) with:

```bash
# SMA-694. The route flags are on every row: an HTTPRoute has the same name as its console Service,
# and the names must stay unique per kind at every release length.
ROUTE=(--set httpRoute.enabled=true --set 'httpRoute.parentRefs[0].name=gw'
  --set 'httpRoute.parentRefs[0].sectionName=https')
check "short release"   paigasus                                              --set zones.gateway.enabled=true "${ROUTE[@]}"
check "40-char release" my-very-long-release-name-for-console-xy              --set zones.gateway.enabled=true "${ROUTE[@]}"
check "52-char release" aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  --set zones.gateway.enabled=true "${ROUTE[@]}"
check "53-char release" aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa --set zones.gateway.enabled=true "${ROUTE[@]}"
```

Keep the comment block above the old lines (the "53 is Helm's own ceiling" comment) in place, above the new `# SMA-694.` comment.

- [ ] **Step 2: Run the rows and see them fail**

Run: `$P && /bin/bash charts/paigasus/tests/ingress.sh --set ingress.host=console.example.test; echo rc=$?`
Expected: `rc=1`. The three `route_coupling` rows print `FAIL [...]: 0 HTTPRoute(s), want N; route paths [], want [...]`. The `ingress_count` row and H4 print `ok`.

- [ ] **Step 3: Add the port helper**

In `charts/paigasus/templates/_httproute.tpl`, insert after the first comment block (before the `paigasus.httpRouteEnabled` comment):

```gotemplate
{{/*
paigasus.consoleServicePort: the port NUMBER of each console Service. An HTTPRoute backendRef needs
a number and cannot name the port "http" as the Ingress does. console-service.yaml and
httproute.yaml both read it here, so the two cannot drift (spec D6). The container side (the
containerPort and PORT in console-deployment.yaml) is reached by the port name http.
*/}}
{{- define "paigasus.consoleServicePort" -}}
3000
{{- end -}}
```

In `charts/paigasus/templates/console-service.yaml`, replace `      port: 3000` with:

```yaml
      port: {{ include "paigasus.consoleServicePort" $root }}
```

- [ ] **Step 4: Create `templates/httproute.yaml`**

```gotemplate
{{/* SPDX-License-Identifier: Apache-2.0 */}}
{{- include "paigasus.validate" . -}}
{{- $root := . -}}
{{- if (include "paigasus.httpRouteEnabled" .) }}
{{- $h := .Values.httpRoute | default dict }}
{{- range $id, $z := .Values.zones }}
{{- if $z.enabled }}
{{- $name := include "paigasus.name" (list $root (printf "%s-console" $id)) }}
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: {{ $name }}
  # DO NOT add a filter here or in values. Each console compiles basePath in and serves its FULL
  # path already: a URLRewrite that strips {{ $z.basePath }} breaks every route in this zone, and a
  # hostname rewrite breaks the Server Action origin check.
spec:
  # The route's parents, as the operator wrote them. Each one names the HTTPS listener
  # (sectionName or port), so the console never answers on plain http.
  parentRefs:
    {{- toYaml (dig "parentRefs" list $h) | nindent 4 }}
  hostnames:
    - {{ $root.Values.ingress.host | quote }}
  rules:
    # This zone's own {{ $z.basePath }}/auth/* routes are in-app route handlers under its own
    # prefix, so they need no rule of their own.
    - matches:
        - path:
            type: PathPrefix
            value: {{ $z.basePath }}
      backendRefs:
        - name: {{ $name }}
          port: {{ include "paigasus.consoleServicePort" $root }}
{{- end }}
{{- end }}
{{- end }}
```

- [ ] **Step 5: Run the rows and see them pass; check the goldens and names**

Run: `$P && for b in /bin/bash /opt/homebrew/bin/bash; do for s in ingress names render refusals; do $b charts/paigasus/tests/$s.sh --set ingress.host=console.example.test | tail -1; done; done; git status --short charts/paigasus/tests/golden`
Expected: eight OK lines (`== chart ingress coupling OK ==`, `== chart names OK ==`, `== chart render OK ==`, `== chart refusals OK ==`, twice), and no golden file listed. `names.sh` now reports `OK 11 objects` on each row.

- [ ] **Step 6: Commit**

```bash
$P && git branch --show-current   # must print feature/sma-694-helm-httproute
git add charts/paigasus/templates/_httproute.tpl charts/paigasus/templates/httproute.yaml \
  charts/paigasus/templates/console-service.yaml charts/paigasus/tests/ingress.sh \
  charts/paigasus/tests/names.sh
git commit -m "feat(repo): render one Gateway API HTTPRoute per enabled zone in the Helm chart (SMA-694)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: `httpRoute.annotations`

**Files:**
- Modify: `charts/paigasus/templates/_httproute.tpl` (`paigasus.validateHttpRoute`)
- Modify: `charts/paigasus/templates/httproute.yaml`
- Modify: `charts/paigasus/values.yaml`
- Test: `charts/paigasus/tests/refusals.sh`, `charts/paigasus/tests/ingress.sh`

**Interfaces:**
- Consumes: `ROUTE` (both scripts), `route_shape` (Task 2); the lowercase check in `paigasus.validateHttpRoute` as the insert anchor (Task 1).
- Produces: the annotations checks in `paigasus.validateHttpRoute`, ending with the string-check `fail` line, which Task 4 uses as its insert anchor.

- [ ] **Step 1: Write the failing rows**

In `charts/paigasus/tests/refusals.sh`, directly after the R7 row (`expect_render "httpRoute parentRef with port" ...`), add:

```bash
# R8, R9 (D14). The API server needs string annotation values; --set gives a boolean for "true".
expect_fail "httpRoute annotations not a map" "httpRoute.annotations must be a map" \
  "${ROUTE[@]}" --set httpRoute.annotations=x
expect_fail "httpRoute annotation not a string" "httpRoute.annotations.owner must be a string" \
  "${ROUTE[@]}" --set httpRoute.annotations.owner=true
```

In `charts/paigasus/tests/ingress.sh`, directly after the H4 row (`route_shape "route, httpRoute key absent" ...`), add:

```bash
# H8 (AC10). The annotations go on every HTTPRoute. The empty default renders no annotations key.
route_shape "route, annotations" \
  'gateway:-:{"example.test/owner":"team"} iam:-:{"example.test/owner":"team"}' \
  "${ROUTE[@]}" --set zones.gateway.enabled=true --set 'httpRoute.annotations.example\.test/owner=team'
route_shape "route, no annotations" 'gateway:-:- iam:-:-' "${ROUTE[@]}" --set zones.gateway.enabled=true
```

- [ ] **Step 2: Run the rows and see them fail**

Run: `$P && /bin/bash charts/paigasus/tests/refusals.sh --set ingress.host=console.example.test | grep -E 'annotation'; /bin/bash charts/paigasus/tests/ingress.sh --set ingress.host=console.example.test | grep 'annotations'`
Expected: R8 fails (the render succeeds with a coalesce warning, or refuses with another message). R9 prints `rendered, expected a refusal`. `route, annotations` prints `FAIL ... got "gateway:-:- iam:-:-"`. `route, no annotations` prints `ok`.

- [ ] **Step 3: Add the checks**

In `charts/paigasus/templates/_httproute.tpl`, replace

```gotemplate
{{- fail (printf "ingress.host must be lowercase when httpRoute.enabled is true (Gateway API hostnames are lowercase RFC 1123 names), got %q" $host) -}}
{{- end -}}
```

with

```gotemplate
{{- fail (printf "ingress.host must be lowercase when httpRoute.enabled is true (Gateway API hostnames are lowercase RFC 1123 names), got %q" $host) -}}
{{- end -}}
{{- $ann := dig "annotations" dict $h -}}
{{- if not (or (kindIs "invalid" $ann) (kindIs "map" $ann)) -}}
{{- fail (printf "httpRoute.annotations must be a map, got %s" (kindOf $ann)) -}}
{{- end -}}
{{- range $k, $v := ($ann | default dict) -}}
{{- if not (kindIs "string" $v) -}}
{{- fail (printf "httpRoute.annotations.%s must be a string, got %s; quote it in a values file or use --set-string" $k (kindOf $v)) -}}
{{- end -}}
{{- end -}}
```

- [ ] **Step 4: Render the annotations**

In `charts/paigasus/templates/httproute.yaml`, replace

```gotemplate
metadata:
  name: {{ $name }}
  # DO NOT add a filter
```

with

```gotemplate
metadata:
  name: {{ $name }}
  {{- with (dig "annotations" dict $h) }}
  annotations:
    {{- toYaml . | nindent 4 }}
  {{- end }}
  # DO NOT add a filter
```

- [ ] **Step 5: Add the value**

In `charts/paigasus/values.yaml`, in the `httpRoute` block, replace

```yaml
  parentRefs: []
  # DO NOT add a rewrite or any other filter. Each console serves its full basePath.
```

with

```yaml
  parentRefs: []
  # NOT required. Annotations on each HTTPRoute, for example for external-dns. Each value must be
  # a string. The chart does not check the text: a value that names a zone id is your own text.
  annotations: {}
  # DO NOT add a rewrite or any other filter. Each console serves its full basePath.
```

- [ ] **Step 6: Run the rows and see them pass**

Run: `$P && for b in /bin/bash /opt/homebrew/bin/bash; do for s in ingress refusals render; do $b charts/paigasus/tests/$s.sh --set ingress.host=console.example.test | tail -1; done; done`
Expected: six OK lines.

- [ ] **Step 7: Commit**

```bash
$P && git branch --show-current   # must print feature/sma-694-helm-httproute
git add charts/paigasus/templates/_httproute.tpl charts/paigasus/templates/httproute.yaml \
  charts/paigasus/values.yaml charts/paigasus/tests/refusals.sh charts/paigasus/tests/ingress.sh
git commit -m "feat(repo): add httpRoute.annotations to the Helm chart HTTPRoutes (SMA-694)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Route timeouts (`httpRoute.timeouts` and `zones.<id>.console.httpRouteTimeouts`)

**Files:**
- Modify: `charts/paigasus/templates/_httproute.tpl` (three new helpers, and checks in `paigasus.validateHttpRoute`)
- Modify: `charts/paigasus/templates/httproute.yaml`
- Modify: `charts/paigasus/values.yaml` (the `httpRoute` block and `zones.gateway.console`)
- Test: `charts/paigasus/tests/refusals.sh`, `charts/paigasus/tests/ingress.sh`

**Interfaces:**
- Consumes: `ROUTE`, `route_shape`; the annotations string-check in `paigasus.validateHttpRoute` as the insert anchor (Task 3).
- Produces: `paigasus.httpRouteTimeouts` (`(list $root $z)` → YAML of the merged map, or `""`); `paigasus.durationMs` (a duration string → an integer string of milliseconds); `paigasus.validateHttpRouteTimeouts` (`(list <path string> <value>)` → fails or `""`).

- [ ] **Step 1: Write the failing rows**

In `charts/paigasus/tests/refusals.sh`, directly after the R9 row, add:

```bash
# R10-R16 (D13). The chart refuses what the HTTPRoute v1 CRD refuses, before Argo CD sync time.
GW=(--set zones.gateway.enabled=true --set zones.gateway.backend.url=http://gw.example.test:8088)
expect_fail "httpRoute timeouts not a map" "httpRoute.timeouts must be a map" \
  "${ROUTE[@]}" --set httpRoute.timeouts=10s
expect_fail "httpRoute timeouts unknown key" "httpRoute.timeouts has the unknown key requestTimeout" \
  "${ROUTE[@]}" --set httpRoute.timeouts.requestTimeout=10s
expect_fail "httpRoute timeout bad form" "httpRoute.timeouts.request must be a duration" \
  "${ROUTE[@]}" --set-string httpRoute.timeouts.request=10
expect_fail "httpRoute timeout a number" "httpRoute.timeouts.request must be a duration" \
  "${ROUTE[@]}" --set httpRoute.timeouts.request=10
expect_fail "zone timeout bad form" "zones.gateway.console.httpRouteTimeouts.request must be a duration" \
  "${ROUTE[@]}" "${GW[@]}" --set zones.gateway.console.httpRouteTimeouts.request=ten
expect_fail "httpRoute backendRequest longer than request" "zones.iam: the HTTPRoute backendRequest timeout 1m" \
  "${ROUTE[@]}" --set httpRoute.timeouts.request=10s --set httpRoute.timeouts.backendRequest=1m
expect_render "httpRoute request 0s with backendRequest" \
  "${ROUTE[@]}" --set httpRoute.timeouts.request=0s --set httpRoute.timeouts.backendRequest=1m
expect_render "zone timeout on a disabled zone is not checked" \
  "${ROUTE[@]}" --set zones.gateway.enabled=false --set zones.gateway.console.httpRouteTimeouts.request=ten
# Review Focus 1. "ms" is not "m": 1s is longer than 900ms, and 59s999ms is shorter than 1m.
expect_fail "httpRoute backendRequest 1s over request 900ms" "zones.iam: the HTTPRoute backendRequest timeout 1s" \
  "${ROUTE[@]}" --set httpRoute.timeouts.request=900ms --set httpRoute.timeouts.backendRequest=1s
expect_render "httpRoute backendRequest 59s999ms under request 1m" \
  "${ROUTE[@]}" --set httpRoute.timeouts.request=1m --set httpRoute.timeouts.backendRequest=59s999ms
# Review Focus 2. The gateway zone's 10m default is its merged request, so a chart-wide
# backendRequest of 20m is refused for that zone only.
expect_fail "chart backendRequest over the zone default" "zones.gateway: the HTTPRoute backendRequest timeout 20m" \
  "${ROUTE[@]}" "${GW[@]}" --set httpRoute.timeouts.backendRequest=20m
```

In `charts/paigasus/tests/ingress.sh`, the gateway rule now gets the `10m` default, so update the two H8 rows of Task 3. Replace `'gateway:-:{"example.test/owner":"team"} iam:-:{"example.test/owner":"team"}'` with `'gateway:{"request":"10m"}:{"example.test/owner":"team"} iam:-:{"example.test/owner":"team"}'`, and in the row `route, no annotations` replace `'gateway:-:- iam:-:-'` with `'gateway:{"request":"10m"}:- iam:-:-'`. Then, directly after the H8 rows, add:

```bash
# H5-H7 (AC9). The gateway zone has a 10m request default; the iam zone has none. A zone key wins
# over the same chart key, per key. With no key set, no rule has a timeouts key.
route_shape "route, default timeouts" 'gateway:{"request":"10m"}:- iam:-:-' \
  "${ROUTE[@]}" --set zones.gateway.enabled=true
route_shape "route, chart timeouts merge per key" \
  'gateway:{"backendRequest":"20s","request":"10m"}:- iam:{"backendRequest":"20s","request":"30s"}:-' \
  "${ROUTE[@]}" --set zones.gateway.enabled=true \
  --set httpRoute.timeouts.request=30s --set httpRoute.timeouts.backendRequest=20s
route_shape "route, no timeouts set" 'gateway:-:- iam:-:-' "${ROUTE[@]}" --set zones.gateway.enabled=true \
  --set zones.gateway.console.httpRouteTimeouts=null
# Review Focus 3. An empty zone key is "not set": the chart key applies.
route_shape "route, empty zone key keeps the chart key" \
  'gateway:{"request":"30s"}:- iam:{"request":"30s"}:-' "${ROUTE[@]}" --set zones.gateway.enabled=true \
  --set httpRoute.timeouts.request=30s --set zones.gateway.console.httpRouteTimeouts.request=""
# Review Focus 5. A release made before this change, upgraded with --reuse-values and
# httpRoute.enabled=true, has no timeouts or annotations key.
route_shape "route, timeouts and annotations keys absent" 'gateway:{"request":"10m"}:- iam:-:-' \
  "${ROUTE[@]}" --set zones.gateway.enabled=true --set httpRoute.timeouts=null --set httpRoute.annotations=null
```

- [ ] **Step 2: Run the rows and see them fail**

Run: `$P && /bin/bash charts/paigasus/tests/refusals.sh --set ingress.host=console.example.test | grep -E 'timeout|backendRequest'; /bin/bash charts/paigasus/tests/ingress.sh --set ingress.host=console.example.test | grep -E 'timeouts|zone key'`
Expected: every new `expect_fail` row prints `FAIL`. `route, default timeouts`, `route, chart timeouts merge per key`, `route, empty zone key keeps the chart key`, `route, timeouts and annotations keys absent` and the two updated H8 rows print `FAIL`. `route, no timeouts set` prints `ok`.

- [ ] **Step 3: Add the three helpers**

In `charts/paigasus/templates/_httproute.tpl`, insert directly before the comment that starts `paigasus.validateHttpRoute: the refusals`:

```gotemplate
{{/*
paigasus.httpRouteTimeouts takes (list $root $z) and yields the merged timeouts of one zone's rule
as YAML, or "" when no key is set (spec D13). A non-empty key in
zones.<id>.console.httpRouteTimeouts wins over the same key in httpRoute.timeouts. The merge is
per key, and an empty string is "not set". Both maps are read with dig: a release made before this
change has neither key.
*/}}
{{- define "paigasus.httpRouteTimeouts" -}}
{{- $root := index . 0 -}}
{{- $z := index . 1 -}}
{{- $h := $root.Values.httpRoute | default dict -}}
{{- $chart := dig "timeouts" dict $h | default dict -}}
{{- $zone := dig "console" "httpRouteTimeouts" dict $z | default dict -}}
{{- $out := dict -}}
{{- range $k := list "request" "backendRequest" -}}
{{- $v := index $zone $k | default (index $chart $k) -}}
{{- if $v -}}{{- $_ := set $out $k ($v | toString) -}}{{- end -}}
{{- end -}}
{{- if $out -}}{{- toYaml $out -}}{{- end -}}
{{- end -}}

{{/*
paigasus.durationMs takes a Gateway API Duration string (GEP-2257) that already matches
^([0-9]{1,5}(h|m|s|ms)){1,4}$, and yields its value in milliseconds. "ms" comes first in the
alternation, so "10ms" is not read as ten minutes.
*/}}
{{- define "paigasus.durationMs" -}}
{{- $total := 0 -}}
{{- range $part := regexFindAll "[0-9]+(ms|h|m|s)" . -1 -}}
{{- $digits := regexFind "^[0-9]+" $part -}}
{{- $n := int $digits -}}
{{- $unit := trimPrefix $digits $part -}}
{{- if eq $unit "h" -}}{{- $total = add $total (mul $n 3600000) -}}{{- end -}}
{{- if eq $unit "m" -}}{{- $total = add $total (mul $n 60000) -}}{{- end -}}
{{- if eq $unit "s" -}}{{- $total = add $total (mul $n 1000) -}}{{- end -}}
{{- if eq $unit "ms" -}}{{- $total = add $total $n -}}{{- end -}}
{{- end -}}
{{- $total -}}
{{- end -}}

{{/*
paigasus.validateHttpRouteTimeouts takes (list <path> <value>) and refuses a timeouts map that the
HTTPRoute v1 CRD would refuse at apply time (spec § 4.4, checks 5 to 7). An absent value (nil)
passes: it means "not set". So does an empty string for a key.
*/}}
{{- define "paigasus.validateHttpRouteTimeouts" -}}
{{- $path := index . 0 -}}
{{- $m := index . 1 -}}
{{- if not (kindIs "invalid" $m) -}}
{{- if not (kindIs "map" $m) -}}
{{- fail (printf "%s must be a map with the optional keys request and backendRequest, got %s" $path (kindOf $m)) -}}
{{- end -}}
{{- range $k, $v := $m -}}
{{- if not (has $k (list "request" "backendRequest")) -}}
{{- fail (printf "%s has the unknown key %s; the allowed keys are request and backendRequest" $path $k) -}}
{{- end -}}
{{- if not (or (kindIs "invalid" $v) (and (kindIs "string" $v) (eq $v ""))) -}}
{{- if not (and (kindIs "string" $v) (regexMatch "^([0-9]{1,5}(h|m|s|ms)){1,4}$" (toString $v))) -}}
{{- fail (printf "%s.%s must be a duration such as 30s, 10m or 1h, got %s %q" $path $k (kindOf $v) (toString $v)) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}
```

- [ ] **Step 4: Call the checks from `paigasus.validateHttpRoute`**

In the same file, replace

```gotemplate
{{- fail (printf "httpRoute.annotations.%s must be a string, got %s; quote it in a values file or use --set-string" $k (kindOf $v)) -}}
{{- end -}}
{{- end -}}
```

with

```gotemplate
{{- fail (printf "httpRoute.annotations.%s must be a string, got %s; quote it in a values file or use --set-string" $k (kindOf $v)) -}}
{{- end -}}
{{- end -}}
{{- include "paigasus.validateHttpRouteTimeouts" (list "httpRoute.timeouts" (dig "timeouts" nil $h)) -}}
{{- $root := . -}}
{{- range $id, $z := .Values.zones -}}
{{- if $z.enabled -}}
{{- include "paigasus.validateHttpRouteTimeouts" (list (printf "zones.%s.console.httpRouteTimeouts" $id) (dig "console" "httpRouteTimeouts" nil $z)) -}}
{{- $t := include "paigasus.httpRouteTimeouts" (list $root $z) | fromYaml -}}
{{- if and $t.request $t.backendRequest -}}
{{- $r := include "paigasus.durationMs" $t.request | int64 -}}
{{- $b := include "paigasus.durationMs" $t.backendRequest | int64 -}}
{{- if and (ne $r 0) (gt $b $r) -}}
{{- fail (printf "zones.%s: the HTTPRoute backendRequest timeout %s is longer than the request timeout %s; the HTTPRoute CRD refuses it (a request of 0s means no timeout and allows any backendRequest)" $id $t.backendRequest $t.request) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}
```

The chart-wide map is checked once. Only an ENABLED zone's own map is checked and merged: a disabled zone leaves no trace (row "zone timeout on a disabled zone is not checked").

- [ ] **Step 5: Render the timeouts**

In `charts/paigasus/templates/httproute.yaml`, replace

```gotemplate
{{- $name := include "paigasus.name" (list $root (printf "%s-console" $id)) }}
---
```

with

```gotemplate
{{- $name := include "paigasus.name" (list $root (printf "%s-console" $id)) }}
{{- $timeouts := include "paigasus.httpRouteTimeouts" (list $root $z) }}
---
```

and replace

```gotemplate
          port: {{ include "paigasus.consoleServicePort" $root }}
{{- end }}
```

with

```gotemplate
          port: {{ include "paigasus.consoleServicePort" $root }}
      {{- with $timeouts }}
      timeouts:
        {{- . | nindent 8 }}
      {{- end }}
{{- end }}
```

- [ ] **Step 6: Add the values**

In `charts/paigasus/values.yaml`, in the `httpRoute` block, replace

```yaml
  annotations: {}
```

with

```yaml
  annotations: {}
  # NOT required. Gateway API HTTPRouteTimeouts for the rule of every enabled zone: request and
  # backendRequest, each a duration such as 30s, 10m or 1h (form ([0-9]{1,5}(h|m|s|ms)){1,4}).
  # 0s means no timeout. backendRequest must not be longer than request, unless request is 0s.
  # A non-empty key in zones.<id>.console.httpRouteTimeouts wins over the same key here.
  # Needs Gateway API v1.2.0 or later on the cluster. See docs/ops/RUNBOOK-chart.md § 11.
  timeouts: {}
```

In the `zones.gateway.console` block, replace

```yaml
        tag: "0.2.0"
      replicas: 2
    backend:
      # NOT deployed by this chart (SMA-513 Task 11b).
```

with

```yaml
        tag: "0.2.0"
      replicas: 2
      # Used only when httpRoute.enabled is true. The chat relay streams text/event-stream for
      # longer than the 15 s default route timeout of Envoy, so this rule gets a longer request
      # timeout. 10m still ends a hung stream. Same shape as httpRoute.timeouts; a key here wins.
      # See docs/ops/RUNBOOK-chart.md § 11.
      httpRouteTimeouts:
        request: 10m
    backend:
      # NOT deployed by this chart (SMA-513 Task 11b).
```

(The iam zone gets no such key.)

- [ ] **Step 7: Run all chart scripts; check the goldens**

Run: `$P && for b in /bin/bash /opt/homebrew/bin/bash; do for s in charts/paigasus/tests/*.sh; do $b $s --set ingress.host=console.example.test | tail -1; done; done; git status --short charts/paigasus/tests/golden`
Expected: seven OK lines per bash (ca-bundle, env, ingress coupling, maps, names, refusals, render), and no golden file listed.

- [ ] **Step 8: Commit**

```bash
$P && git branch --show-current   # must print feature/sma-694-helm-httproute
git add charts/paigasus/templates/_httproute.tpl charts/paigasus/templates/httproute.yaml \
  charts/paigasus/values.yaml charts/paigasus/tests/refusals.sh charts/paigasus/tests/ingress.sh
git commit -m "feat(repo): add HTTPRoute timeouts with a 10m chat default to the Helm chart (SMA-694)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: The third golden file

**Files:**
- Modify: `charts/paigasus/tests/render.sh:4-6, 64-65`
- Create: `charts/paigasus/tests/golden/iam-and-gateway-httproute.yaml`

**Interfaces:**
- Consumes: the complete template from Tasks 2-4.
- Produces: a byte pin of the HTTPRoute shape. Task 7's mutations use it.

- [ ] **Step 1: Add the row and see it fail**

In `charts/paigasus/tests/render.sh`, replace the header comment (lines 4-6)

```bash
# Golden render of the two valid subsets. --kube-version is pinned because helm's default moves
# with the binary, and these files are a byte pin. Re-baseline deliberately with --update; a
# golden change is a reviewable event, never a mechanical edit to clear a red.
```

with

```bash
# Golden render of the two valid subsets, and of the HTTPRoute mode (SMA-694). --kube-version is
# pinned because helm's default moves with the binary, and these files are a byte pin.
# Re-baseline deliberately with --update; a golden change is a reviewable event, never a
# mechanical edit to clear a red.
```

Then, after `render_one "iam-and-gateway" --set zones.gateway.enabled=true`, add:

```bash
# SMA-694. A byte pin of the HTTPRoute shape: the Ingress off, one parentRef with a namespace, a
# chart-wide backendRequest merged with the gateway zone's 10m request default, one annotation.
render_one "iam-and-gateway-httproute" --set zones.gateway.enabled=true \
  --set ingress.enabled=false --set httpRoute.enabled=true \
  --set 'httpRoute.parentRefs[0].name=edge' --set 'httpRoute.parentRefs[0].namespace=kube-system' \
  --set 'httpRoute.parentRefs[0].sectionName=https' \
  --set httpRoute.timeouts.backendRequest=30s \
  --set 'httpRoute.annotations.example\.test/owner=platform'
```

Run: `$P && /bin/bash charts/paigasus/tests/render.sh --set ingress.host=console.example.test; echo rc=$?`
Expected: `rc=1` and `FAIL [iam-and-gateway-httproute]: no golden file at ...`.

- [ ] **Step 2: Create the golden and read it**

Run: `$P && /bin/bash charts/paigasus/tests/render.sh --update --set ingress.host=console.example.test && git status --short charts/paigasus/tests/golden && grep -n -A40 'kind: HTTPRoute' charts/paigasus/tests/golden/iam-and-gateway-httproute.yaml | head -90`
Expected: `git status` lists ONLY `?? charts/paigasus/tests/golden/iam-and-gateway-httproute.yaml`. If an old golden is listed as modified, stop: AC4 is broken. The file holds no `kind: Ingress`. It holds two HTTPRoutes. The gateway route has `timeouts:` with `backendRequest: 30s` and `request: 10m`. The iam route has `timeouts:` with only `backendRequest: 30s`. Both have `annotations: example.test/owner: platform`, `hostnames: - "console.example.test"`, and `parentRefs` with `name: edge`, `namespace: kube-system`, `sectionName: https`.

- [ ] **Step 3: Run it green under both bashes**

Run: `$P && /bin/bash charts/paigasus/tests/render.sh --set ingress.host=console.example.test | tail -1; /opt/homebrew/bin/bash charts/paigasus/tests/render.sh --set ingress.host=console.example.test | tail -1`
Expected: `== chart render OK ==` twice.

- [ ] **Step 4: Commit**

```bash
$P && git branch --show-current   # must print feature/sma-694-helm-httproute
git add charts/paigasus/tests/render.sh charts/paigasus/tests/golden/iam-and-gateway-httproute.yaml
git commit -m "test(repo): pin the Helm chart HTTPRoute render with a third golden file (SMA-694)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: The docs

**Files:**
- Modify: `charts/paigasus/values.yaml:1-10, 104-108`
- Modify: `charts/CLAUDE.md`
- Modify: `charts/paigasus/README.md`
- Modify: `docs/ops/RUNBOOK-chart.md`

**Interfaces:**
- Consumes: the value names, the refusal messages and the defaults from Tasks 1-4.
- Produces: docs only. The test for this task is the full chart battery (no golden may move, because `values.yaml` comments do not render) and a grep for the removed SMA-694 forward references.

Write every new sentence in ASD-STE100 Simplified Technical English (short sentences, active voice, no idiom), as the text around it does.

- [ ] **Step 1: `values.yaml` header and the `ingress.enabled` comment**

Replace lines 3-9 (from `# ONE values block.` to `# operator keeps by hand.`) with:

```yaml
# ONE values block. Six projections derive from `zones` and from nothing else: the route path
# rules (the Ingress rules, and the HTTPRoute rules when httpRoute.enabled is true),
# PAIGASUS_ZONES, PAIGASUS_SERVICES, PAIGASUS_IAM_GRPC_URL, the console Deployments and the
# backend Deployments. A zone's `enabled` flag governs ALL of them together — that is what makes
# "routable but unadvertised" unrepresentable rather than merely checked (spec D6).
#
# Exception (SMA-695, spec D2): with ingress.enabled false AND httpRoute.enabled false the chart
# renders no route path rules, and the operator's own route is a projection of `zones` that the
# operator keeps by hand.
```

Replace the `ingress.enabled` comment lines 105-108 with:

```yaml
  enabled: true          # false: the chart renders no Ingress. Then set httpRoute.enabled for the
                         # chart-owned HTTPRoute (below), or route https://<host><basePath> to each
                         # enabled zone's console Service yourself: port name http, no rewrite,
                         # Host forwarded unchanged, TLS ended in front of the Services.
                         # See docs/ops/RUNBOOK-chart.md § 11. A boolean: a quoted "false" is refused.
```

- [ ] **Step 2: `charts/CLAUDE.md`**

Replace the rewrite bullet with:

```markdown
- **The ingress and the HTTPRoute have no rewrite, on purpose.** Each console has its `basePath`
  compiled in. A rewrite that removes `/iam` breaks every route in that zone. Do not add one to
  `templates/ingress.yaml`, `templates/httproute.yaml` or `values.yaml`. The HTTPRoute has no
  `filters` at all: a hostname rewrite breaks Next's Server Action origin check.
```

Replace the "One values block renders three projections" bullet with:

```markdown
- **One values block renders three projections that must agree:** `PAIGASUS_ZONES`,
  `PAIGASUS_SERVICES` and the route rules (the Ingress rules, and the HTTPRoute rules when
  `httpRoute.enabled` is true). All three come from `zones.<id>.enabled` through one `range`. Do
  not write a zone literally in a template. For the Ingress, `repo:helm-render` check 1 and
  check 2 fail on it. `repo:helm-render` renders with `httpRoute` off and sees no HTTPRoute
  (SMA-694 D10): for `templates/httproute.yaml`, `tests/ingress.sh` rows H1 and H2 fail on it
  instead. With `ingress.enabled: false` and `httpRoute.enabled: false` (SMA-695) the chart
  renders no route rules, so only two projections remain, and the operator keeps the route by
  hand.
```

After the `paigasus.ingressEnabled` bullet, add:

```markdown
- **Read `httpRoute.enabled` only through `paigasus.httpRouteEnabled`** (`templates/_httproute.tpl`).
  A release made before SMA-694 has no `httpRoute` map under `--reuse-values`, and a plain
  `.Values.httpRoute.enabled` is then a nil-pointer error. No comment in `httproute.yaml` may
  contain the word "gateway": the H2 row greps the iam-only render for it (after it removes the
  `apiVersion: gateway.networking.k8s.io/v1` lines).
```

In the fixture bullet, after the sentence that ends `Chart.yaml` holds one: `app-version-unreleased`.`, add: `` `templates/_httproute.tpl` has no copy: a fixture chart gets it from the live chart. ``

- [ ] **Step 3: `charts/paigasus/README.md`**

a. In "One values block, six projections", replace the bullet `- the ingress path rule for that zone (\`templates/ingress.yaml\`)` with:

```markdown
- the route path rule for that zone: the Ingress rule (`templates/ingress.yaml`), and the
  HTTPRoute (`templates/httproute.yaml`) when `httpRoute.enabled` is true
```

Replace the sentence `` `tests/ingress.sh` and `tests/maps.sh` assert this directly. `` with `` `tests/ingress.sh` (also for the HTTPRoute) and `tests/maps.sh` assert this directly. ``

Replace the paragraph that starts `**Exception: \`ingress.enabled: false\` (SMA-695).**` with:

```markdown
**Exception: `ingress.enabled: false` and `httpRoute.enabled: false` (SMA-695).** The chart then
renders no route, so the first projection is gone. The operator's own route is a projection of
`zones` that the operator keeps by hand. A route to a disabled zone points to a deleted Service.
An enabled zone with no route gives a 404 for its `PAIGASUS_ZONES` link. With
`httpRoute.enabled: true` (SMA-694) the chart renders the route again, and D6 holds.
```

b. In "The refusals", before the paragraph that starts `` `tests/refusals.sh` renders each refusal case ``, add:

```markdown
- **A bad `httpRoute` block (SMA-694).** `paigasus.validateHttpRoute` in
  `templates/_httproute.tpl` refuses:
  - `httpRoute` that is not a map: `httpRoute must be a map`;
  - `httpRoute.enabled` that is not a boolean: `httpRoute.enabled must be true or false`;
  - in route mode, an absent, null, empty or non-list `parentRefs`:
    `httpRoute.parentRefs is required when httpRoute.enabled is true`. An absent key counts as
    empty, so a `--reuse-values` upgrade that turns the route on without it fails;
  - a parentRef that is not a map or has no string `name`:
    `httpRoute.parentRefs[<i>] must be a map with a non-empty name`;
  - a parentRef with neither `sectionName` nor `port`:
    `httpRoute.parentRefs[<i>] must set sectionName or port`. Such a parentRef attaches to every
    listener, a plain HTTP one included, and the console then answers on `http://`;
  - an `ingress.host` with an upper-case letter: `ingress.host must be lowercase when
    httpRoute.enabled is true`. The API server refuses such an HTTPRoute hostname;
  - `httpRoute.annotations` that is not a map, or a value that is not a string:
    `httpRoute.annotations must be a map`, `httpRoute.annotations.<key> must be a string`;
  - a timeouts map (`httpRoute.timeouts` or an enabled zone's
    `zones.<id>.console.httpRouteTimeouts`) that is not a map, has a key other than `request`
    and `backendRequest`, or has a value that is not a Gateway API duration:
    `<path> must be a map`, `<path> has the unknown key <key>`, `<path>.<key> must be a
    duration`;
  - a merged `backendRequest` longer than the merged `request`, when `request` is not `0s`:
    `zones.<id>: the HTTPRoute backendRequest timeout <b> is longer than the request timeout <r>`.

  The HTTPRoute CRD refuses the same values, but only at apply time. In Argo CD that is a sync
  error that is easy to miss.
```

c. Replace the section "Running without an Ingress controller" body with:

```markdown
Set `ingress.enabled: false` when the cluster has no Ingress controller, for example a cluster
that uses Gateway API. The chart then renders no Ingress, and `ingress.tlsSecretName` is not
required. `ingress.host` stays required, because it feeds `PAIGASUS_PUBLIC_ORIGIN` and the OIDC
redirect URIs. `ingress.className` and `ingress.annotations` then do nothing.

`templates/ingress.yaml` wraps its body in `paigasus.ingressEnabled` (`templates/_helpers.tpl`).
`tests/ingress.sh` and `tests/refusals.sh` hold the SMA-695 rows.

**Gateway API: the chart-owned HTTPRoute (SMA-694).** Set `httpRoute.enabled: true` and
`httpRoute.parentRefs`. The chart then renders one `gateway.networking.k8s.io/v1` HTTPRoute for
each enabled zone, with the same name as the zone's console Service. Each route has one rule: a
`PathPrefix` match on the zone's `basePath`, sent to the console Service on port 3000, with no
filter. Its `hostnames` is `[<ingress.host>]`. The two switches are independent: both can be on
during a cut-over. The prerequisites:

- the Gateway API CRDs, standard channel, with `HTTPRoute` served at `v1`. `timeouts` needs
  Gateway API v1.2.0 or later;
- a Gateway with an HTTPS listener that ends TLS for `ingress.host` and allows the release
  namespace in `allowedRoutes`. Each parentRef names that listener with `sectionName` or `port`;
- an HTTP-to-HTTPS `RequestRedirect` route on any plain HTTP listener of that Gateway;
- create permission for the deployer on `gateway.networking.k8s.io/httproutes`. The standard
  Gateway API install adds no aggregated rules to the `admin` and `edit` ClusterRoles;
- an Argo CD AppProject resource whitelist that allows the `HTTPRoute` kind.

`httpRoute.timeouts` sets `request` and `backendRequest` on the rule of every enabled zone.
`zones.<id>.console.httpRouteTimeouts` sets them for one zone, and a key there wins. The gateway
zone has the default `request: 10m`, because its chat relay streams for longer than the 15 s
default route timeout of Envoy. `httpRoute.annotations` goes on each HTTPRoute, for example for
external-dns. `tests/ingress.sh` (rows H1-H8), `tests/refusals.sh`, `tests/names.sh` and the
third golden hold the SMA-694 rows. The chart does not check that the cluster has the CRDs: a
`.Capabilities` gate would fail `helm template` in CI and in the Argo CD repo server.

You can also keep both switches off and write the route yourself. The routing contract is in
`docs/ops/RUNBOOK-chart.md` § 11.
```

d. In "The golden files", replace the first sentence ``tests/golden/iam-only.yaml` and `tests/golden/iam-and-gateway.yaml` are a byte-exact pin of `helm template` for the two valid zone subsets,`` with ``tests/golden/iam-only.yaml` and `tests/golden/iam-and-gateway.yaml` are a byte-exact pin of `helm template` for the two valid zone subsets, and `tests/golden/iam-and-gateway-httproute.yaml` pins the HTTPRoute mode (SMA-694),``, and replace `` `tests/render.sh --update` re-baselines both files `` with `` `tests/render.sh --update` re-baselines all three files ``.

- [ ] **Step 4: `docs/ops/RUNBOOK-chart.md`**

a. § 1: replace the `ingress.enabled` row with:

```markdown
| `ingress.enabled` | no | Default `true`. `false`: the chart renders no Ingress. Then set `httpRoute.enabled`, or route the traffic yourself (§ 11). It must be a boolean |
```

After the `ingress.annotations` row, add:

```markdown
| `httpRoute.enabled` | no | Default `false`. `true`: the chart renders one Gateway API HTTPRoute for each enabled zone (§ 11). Independent of `ingress.enabled`. It must be a boolean. `ingress.host` must then be lowercase |
| `httpRoute.parentRefs` | when `httpRoute.enabled` is true | A non-empty list of Gateway API `ParentReference` objects, rendered as written. Each entry needs a `name`, and a `sectionName` or `port` that selects the HTTPS listener |
| `httpRoute.timeouts` | no | `request` and `backendRequest` (durations such as `30s`, `10m`, `1h`) for the rule of every enabled zone. Default `{}` (§ 11, "Route timeouts") |
| `httpRoute.annotations` | no | Annotations on each HTTPRoute, for example for external-dns. String values only. Default `{}` |
| `zones.<id>.console.httpRouteTimeouts` | no | The same shape as `httpRoute.timeouts`, for one zone. A key here wins. The gateway zone default is `request: 10m` |
```

b. § 2: replace the intro sentence `The last two items in this list are in \`templates/_audience.tpl\` instead.` with `Two other files hold refusals: the two `oidc.authorizationAudience` items are in `templates/_audience.tpl`, and the `httpRoute` items are in `templates/_httproute.tpl` (`paigasus.validateHttpRoute`, which `paigasus.validate` calls).` Keep the next sentence (about `console-env-configmap.yaml`) and change its "them" to "the `_audience.tpl` checks". At the end of the list, add:

```markdown
- `httpRoute` that is not a map, or `httpRoute.enabled` that is not a boolean;
- with `httpRoute.enabled: true`: an absent, null, empty or non-list `httpRoute.parentRefs`; a
  parentRef without a string `name`; a parentRef with neither `sectionName` nor `port`; an
  `ingress.host` with an upper-case letter;
- with `httpRoute.enabled: true`: `httpRoute.annotations` that is not a map, or a value that is
  not a string;
- with `httpRoute.enabled: true`: a timeouts map (`httpRoute.timeouts`, or the
  `httpRouteTimeouts` of an enabled zone) that is not a map, has a key other than `request` and
  `backendRequest`, or has a value that is not a duration; and a merged `backendRequest` longer
  than the merged `request` when `request` is not `0s`.
```

c. § 4: replace the paragraph `**D6 and \`ingress.enabled: false\` (SMA-695).** ...` with:

```markdown
**D6 and `ingress.enabled: false` (SMA-695).** With the Ingress off and `httpRoute.enabled: false`,
the chart renders no routing. Your own route then takes the place of the ingress rule, and you keep
it by hand. Change the route in the same change as any `zones.<id>.enabled` edit. A route to a
disabled zone points to a deleted Service. An enabled zone with no route gives a 404 for its link.

**D6 and `httpRoute.enabled: true` (SMA-694).** The chart renders one HTTPRoute for each enabled
zone, so D6 holds. Note the difference from the Ingress: the Ingress is one object, and a zone
change modifies it. A disabled zone's HTTPRoute is a whole object that the change DELETES. Without
Argo CD prune, the old route stays, and the zone stays routed while `PAIGASUS_ZONES` drops it. That
is the state D6 forbids. Enable prune, or delete the route by hand.
```

d. § 8: after the sentence `Set \`ingress.className\` to your controller's class.`, add: `Gateway API through `httpRoute.enabled` is the other supported path (§ 11). The kind job does not test it: the proof for it is a manual check on the target cluster (SMA-694 spec § 5.4).`

e. § 11: after the first paragraph (it ends `reports the Application as Progressing forever.`), add:

```markdown
**The chart-owned HTTPRoute (SMA-694).** Set `httpRoute.enabled: true` and give
`httpRoute.parentRefs`, for example
`[{name: cilium-gateway, namespace: kube-system, sectionName: https}]`. The chart renders one
HTTPRoute for each enabled zone, with the same name as that zone's console Service, and meets the
routing contract below. Before you turn it on, make sure that:

1. the Gateway API CRDs are installed, standard channel, with `HTTPRoute` served at `v1`
   (v1.2.0 or later for `timeouts`);
2. the listener that each parentRef names is HTTPS and ends TLS for `ingress.host`;
3. the listener allows the release namespace in `allowedRoutes`. The chart cannot see this;
4. each plain HTTP listener of the Gateway has an HTTP-to-HTTPS `RequestRedirect` route;
5. the deployer can create `gateway.networking.k8s.io/httproutes`, and the Argo CD AppProject
   allows the `HTTPRoute` kind.

Verify each route after the sync. First, `kubectl -n <ns> get httproute -o jsonpath='{range
.items[*]}{.metadata.name}{" "}{range .status.parents[*].conditions[*]}{.type}={.status}{"
"}{end}{"\n"}{end}'` must show `Accepted=True` and `ResolvedRefs=True` for each route. Then send a
request through the Gateway address:
`curl -sS -o /dev/null -w '%{http_code}\n' --resolve <host>:443:<gateway-ip> https://<host>/iam/`.
A status alone does not prove that the route carries traffic.

**Route timeouts.** Envoy, which Cilium's Gateway API uses, ends a request after 15 s by default.
The gateway console relays chat as `text/event-stream`, and a chat stream can take longer. So the
gateway zone's rule has `request: 10m` by default (`zones.gateway.console.httpRouteTimeouts`). The
iam zone has no default. `httpRoute.timeouts` sets `request` and `backendRequest` for every zone.
A non-empty key in a zone's `httpRouteTimeouts` wins over the same key, key by key. `0s` means "no
timeout". Do not use it: a hung upstream connection then stays open with no limit. The chart
refuses a `backendRequest` longer than `request`, unless `request` is `0s`. `timeouts` is an
"Extended" Gateway API feature. An implementation that does not support it can refuse the route
(`Accepted: False`). Nobody measured the Cilium version on the target cluster for this. After you
turn on the route, send a gateway console chat that streams for more than 15 seconds, and make sure
that it completes.

**Annotations.** `httpRoute.annotations` goes on each HTTPRoute. The chart does not check the
text. A value that names a zone id is your own text, and it can name a disabled zone.

**Argo CD health of an HTTPRoute (residual).** The Argo CD version on the target cluster is not
known. Some Argo CD versions have no health check for HTTPRoute. Argo CD then shows each HTTPRoute
as Healthy at once, and a route that the Gateway refuses does not show as Degraded. Versions with a
check can show a route that no Gateway reports on (for example, a wrong parentRef) as Progressing
forever. In both cases, check `status.parents` by hand, as above.
```

f. § 11 "Cut-over on a release that has a live Ingress", step 1: replace `1. Create the new route and verify it.` with `1. Set `httpRoute.enabled: true` (and `httpRoute.parentRefs`), sync, and verify the route with `status.parents` and a `curl --resolve` request through the Gateway address (see above).` Keep the rest of step 1 (the cert-manager text) as it is.

g. § 11: replace the last line `SMA-694 will add a chart-owned Gateway API HTTPRoute.` with:

```markdown
**Migration from a hand-written route to the chart route.**

1. Look for a hand-written HTTPRoute with the name of a console Service
   (`kubectl -n <ns> get httproute`). The chart route uses that name. If one exists, rename it or
   delete it first. Otherwise `helm upgrade` fails with "invalid ownership metadata".
2. Set `httpRoute.enabled: true` and `httpRoute.parentRefs`, and sync.
3. Delete the hand-written route. Two routes with the same host and the same `PathPrefix` tie, and
   Gateway API gives the tie to the OLDEST route. While the old route exists, the chart route shows
   `Accepted` but carries no traffic.
4. Verify with a request through the Gateway address (`curl --resolve`), not only with
   `status.parents`.
```

- [ ] **Step 5: Check the docs**

Run: `$P && grep -n "SMA-694 will\|SMA-694 (a chart-owned HTTPRoute) will" charts/paigasus/README.md docs/ops/RUNBOOK-chart.md charts/paigasus/values.yaml; for s in charts/paigasus/tests/*.sh; do /bin/bash $s --set ingress.host=console.example.test | tail -1; done; git status --short charts/paigasus/tests/golden`
Expected: the grep prints nothing (the three SMA-695 forward references are gone). Seven OK lines. No golden file listed.

- [ ] **Step 6: Commit**

```bash
$P && git branch --show-current   # must print feature/sma-694-helm-httproute
git add charts/paigasus/values.yaml charts/CLAUDE.md charts/paigasus/README.md docs/ops/RUNBOOK-chart.md
git commit -m "docs(repo): document the Helm chart HTTPRoute, its timeouts and the cut-over (SMA-694)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Gates and the manual mutation record

**Files:**
- Modify (temporarily, then restore): the files that each mutation names
- Modify: this plan, section "Mutation record" at the end

**Interfaces:**
- Consumes: everything from Tasks 1-6.
- Produces: the mutation record and the gate results, which the PR body quotes (spec § 5.2, § 5.3), and the unchecked manual checklist of spec § 5.4, which the PR body copies.

- [ ] **Step 1: Run the gates**

Run each, and record the exit status:

```bash
$P && /bin/bash ci/helm-render/run.sh; echo rc=$?
$P && /bin/bash ci/helm-render/run.sh --self-test; echo rc=$?
$P && /bin/bash ci/helm-render/run.sh --negative-control; echo rc=$?
$P && /opt/homebrew/bin/bash ci/actionlint/run.sh; echo rc=$?
$P && moon run repo:affected-smoke; echo rc=$?
```

Expected: `rc=0` for the three `helm-render` runs and for `affected-smoke`. For actionlint, read its pipe preflight line first. If it reports a pipe under 8192 bytes, rc 2 is a host condition (root `CLAUDE.md`, "This development Mac only"), not a finding: record it and let CI give the verdict. If `affected-smoke` hangs under Homebrew bash, use the bash-only shim of the memory note "affected-smoke hang", or record that CI gives the verdict.

- [ ] **Step 2: Run the mutation battery**

Check first: `$P && git status --short` prints nothing. For each row, make the mutation with Edit, run the named script(s) with `/bin/bash <script> --set ingress.host=console.example.test`, record which rows went red, then restore with `git checkout -- <file>` (the tree was clean, so this restores only the mutation), and check `git status --short` is empty again. A mutation must still parse: remove an `if` together with its `end`. A mutation that fails the render for another reason proves nothing; change it.

| # | Mutation | Script(s) | Must go red |
|---|---|---|---|
| M1 | Delete `templates/httproute.yaml` | ingress.sh, render.sh | H1 rows, `iam-and-gateway-httproute` |
| M2 | In `httproute.yaml`, replace `{{- if (include "paigasus.httpRouteEnabled" .) }}` with `{{- if true }}` | render.sh, ingress.sh | `iam-only`, `iam-and-gateway` (the proof), H4 |
| M3 | Add a literal second rule `- matches: [{path: {type: PathPrefix, value: /iam}}]` with the iam backendRef, outside the `range` | ingress.sh | H1 or H2 |
| M4 | Remove `{{- if $z.enabled }}` and one `{{- end }}` in `httproute.yaml` | ingress.sh | H2 (`route, iam only`) |
| M5 | Add `filters: [{type: URLRewrite, urlRewrite: {path: {type: ReplacePrefixMatch, replacePrefixMatch: /}}}]` to the rule | ingress.sh | H1 |
| M6 | Write `- "console.example.com"` literally in `hostnames` in place of the `ingress.host` value | ingress.sh | H1 (hostnames) |
| M7 | In `paigasus.httpRouteEnabled`, read `.Values.httpRoute.enabled` with no `default dict` | ingress.sh | H4 |
| M8 | Delete the `httpRoute must be a map` check | refusals.sh | R5 |
| M9 | Delete the `httpRoute.enabled must be true or false` check | refusals.sh | R1 |
| M10 | Skip the check when the key is absent: in the validator, replace `dig "parentRefs" list $h` with `dig "parentRefs" (list (dict "name" "x" "port" 443)) $h` | refusals.sh | R2b (R2 stays green: the chart default `[]` is present there) |
| M11 | Delete the `parentRefs is required` check | refusals.sh | R2, R2b, R2c |
| M12 | Delete the parentRef `name` check | refusals.sh | R3 |
| M13 | Delete the `sectionName or port` check | refusals.sh | R4 |
| M14 | Delete the lowercase check | refusals.sh | R6 |
| M15 | Make the listener check `if not $r.sectionName` | refusals.sh | R7 |
| M16 | In `_helpers.tpl` line 82, drop `(include "paigasus.ingressEnabled" .)` from the `and` so the TLS refusal always fires | ingress.sh | H3 |
| M17 | In `httproute.yaml` only, write `port: 3001` | ingress.sh | H1 |
| M18 | Give the HTTPRoute the fixed name `paigasus-console` | names.sh | all four rows |
| M19 | Delete the `timeouts` block from the rule | ingress.sh, render.sh | H5, H6, `iam-and-gateway-httproute` |
| M20 | In `paigasus.httpRouteTimeouts`, swap `$zone` and `$chart` in the `default` line | ingress.sh | H6 |
| M21 | Render `timeouts: {}` when both keys are empty (drop the `with`) | ingress.sh | H5 (iam), H7 |
| M22 | Delete the `httpRouteTimeouts: {request: 10m}` default from `values.yaml` | ingress.sh | H5 |
| M23 | Delete the `annotations` block from `httproute.yaml` | ingress.sh, render.sh | H8, `iam-and-gateway-httproute` |
| M24 | Render `annotations:` also when the map is empty: replace the `with` block by `annotations:` and `{{- toYaml (dig "annotations" dict $h) \| nindent 4 }}` | ingress.sh | H8 (`route, no annotations`) |
| M25 | Delete the annotations kind check / the string check | refusals.sh | R8 / R9 |
| M26 | Delete the timeouts kind check / unknown-key check / form check | refusals.sh | R10 / R11 / R12, R13 |
| M27 | Delete the `backendRequest` order check | refusals.sh | R14, Review Focus 1 and 2 |
| M28 | Remove `(ne $r 0)` from the order check | refusals.sh | R15 |
| M29 | Check and merge every zone, not only enabled zones (drop `if $z.enabled` in the validator) | refusals.sh | R16 |
| M30 | In `paigasus.durationMs`, write the alternation as `(h|m|s|ms)` | refusals.sh | Review Focus 1 |

Each mutation must also leave `iam-only` and `iam-and-gateway` in render.sh green, except M2.

- [ ] **Step 3: Record the results in this plan**

Replace the text under "Mutation record" at the end of this file with a table: mutation number, the rows that went red (with the script's own `FAIL [...]` labels), and "as expected" or the deviation. Add the gate exit statuses of Step 1. If a mutation did not turn its row red, stop and fix the row first (it proves nothing), re-run the WHOLE battery (a later fix can make an earlier row inert), and only then record.

- [ ] **Step 4: Run the full gate graph**

Run: `$P && moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site :http-extractor-envelope :input-liveness :promtool :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci :next-public-free :helm-render :test-e2e --base origin/main --include-relations; echo rc=$?`
Expected: `rc=0`, or failures only in gates that the root `CLAUDE.md` names as bash-version or pipe-size artifacts on this Mac. Diagnose any other failure with the root `CLAUDE.md` moon-diagnosis procedure (Step 0 first). Record the result in the "Mutation record" section.

- [ ] **Step 5: Commit**

```bash
$P && git status --short   # only this plan file may be listed
git add docs/superpowers/plans/2026-09-27-sma-694-helm-httproute.md
git commit -m "docs(repo): record the SMA-694 mutation battery and gate results

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

The PR (feature-factory `open-pr` stage) copies this record into its body, with the heading "Mutation record", and adds the UNCHECKED checklist "Manual verification on the target cluster" with the commands and items (a)-(i) of spec § 5.4, word for word.

---

## Decisions made while unattended

- The spec's Q9 choice stands: the per-zone override and the `10m` default live under `zones.<id>.console.httpRouteTimeouts`. Sven can reverse it at plan review.
- Two extra `ingress.sh` rows beyond the spec: `ingress_count ... 0` for H3 (the spec says "the render has no Ingress", and `route_coupling` does not check that), and `route, no annotations` for the default case of H8.
- R12 has a sibling row for a plain `--set` integer (the spec names that case in its R12 note).
- The mutation record lives in this plan and is copied into the PR body, because an implementer subagent has no other durable place for it.

## Prototype measurements (2026-09-28, helm 3.22.0, scratch copy of the chart)

The code in Tasks 1-4 was rendered on a scratch copy of the chart before this plan was written:

- All seven existing chart scripts pass, and the two goldens are byte-equal (AC4).
- Every refusal row R1-R16 fails with its own message. R2c: helm keeps the `--set` map over the default list and prints `coalesce.go:319: warning: destination for paigasus.httpRoute.parentRefs is a table. Ignoring non-table value ([])`; the chart then refuses it. R8 and R10: helm prints `cannot overwrite table with non table` and keeps the string; the chart refuses it.
- `900ms`/`1s` is refused; `1m`/`59s999ms` renders; `httpRoute.timeouts.backendRequest=20m` with the gateway zone on is refused for `zones.gateway`.
- The H2 needle `gateway` (case-insensitive) finds nothing in the iam-only route render after the `apiVersion` lines are removed, and finds the gateway route when `if $z.enabled` is removed.
- The `route_coupling` and `route_shape` functions of Tasks 2-4 pass under `/bin/bash` 3.2.57 and Homebrew bash 5.x.

## Mutation record

Measured on 2026-09-28 at commit `c21d7dc7` (Tasks 1-6), helm `v3.22.0+g144ca65`, chart scripts
under `/bin/bash` 3.2.57, with `--set ingress.host=console.example.test`.

### Gate results (Step 1)

| Command | rc | Note |
|---|---|---|
| `/bin/bash ci/helm-render/run.sh` | 0 | `helm-render: all checks passed`; render.sh has `ok [iam-and-gateway-httproute]` |
| `/bin/bash ci/helm-render/run.sh --self-test` | 0 | `helm-render self-test passed` |
| `/bin/bash ci/helm-render/run.sh --negative-control` | 0 | `helm-render negative control passed (7 fixtures)` |
| `/opt/homebrew/bin/bash ci/actionlint/run.sh` | 0 | preflight: `pipe capacity 65536 bytes (floor 8192)`, so the local verdict is valid |
| `moon run repo:affected-smoke` | 0 | `affected-graph cascade intact`; no hang |

### Mutation battery (Step 2)

A scratch script applied each mutation to the worktree, ran the named scripts and `render.sh`,
then restored with `git checkout -- charts/paigasus` and checked that `git status --short` was
empty. The tree was clean before and after every row. `render.sh` ran on every row: `iam-only`
and `iam-and-gateway` stayed green on every row except M2, as the plan requires. The battery ran
once, whole. No row needed a fix, so no re-run was necessary.

| # | Rows that went red (`FAIL [...]` labels) | Verdict |
|---|---|---|
| M1 | ingress.sh: `route, iam only`, `route, iam and gateway`, `route, ingress disabled, no TLS Secret` (0 HTTPRoute(s)), and every `route_shape` row except `route, httpRoute key absent` (got `none`); render.sh: `iam-and-gateway-httproute` | as expected |
| M2 | render.sh: `iam-only`, `iam-and-gateway` (the proof); ingress.sh: `route, httpRoute key absent` (H4), and the existing `iam only` row (disabled zone "gateway" appears) | as expected |
| M3 | ingress.sh: `route, iam only`, `route, iam and gateway`, `route, ingress disabled, no TLS Secret` (2 rules, want 1); render.sh: `iam-and-gateway-httproute` | as expected |
| M4 | ingress.sh: `route, iam only` (2 HTTPRoute(s), want 1; and disabled zone "gateway" appears) | as expected |
| M5 | ingress.sh: the three H1/H3 `route_coupling` rows (the rule has filters); render.sh: `iam-and-gateway-httproute` | as expected |
| M6 | ingress.sh: the three `route_coupling` rows (hostnames `['console.example.com']`); render.sh: `iam-and-gateway-httproute` | as expected |
| M7 | ingress.sh: `route, httpRoute key absent` (render fails: `nil pointer evaluating interface {}.enabled`) | as expected |
| M8 | refusals.sh: `httpRoute not a map` (refused, but by `interface conversion: interface {} is bool`, not by the chart message) | as expected |
| M9 | refusals.sh: `httpRoute.enabled a string` (refused, but not with the chart message) | as expected |
| M10 | refusals.sh: `httpRoute parentRefs null` (R2b, rendered); R2 `httpRoute without parentRefs` stayed green | as expected |
| M11 | refusals.sh: `httpRoute without parentRefs`, `httpRoute parentRefs null`, `httpRoute parentRefs a map` | as expected |
| M12 | refusals.sh: `httpRoute parentRef without name` | as expected |
| M13 | refusals.sh: `httpRoute parentRef without listener` | as expected |
| M14 | refusals.sh: `httpRoute host not lowercase` | as expected |
| M15 | refusals.sh: `httpRoute parentRef with port` (R7, refused by the listener check) | as expected |
| M16 | ingress.sh: `route, ingress disabled, no TLS Secret` (H3) and `route, ingress disabled: no Ingress` (refused: `ingress.tlsSecretName is required`) | as expected |
| M17 | ingress.sh: the three `route_coupling` rows (backendRef port 3001 is not a port of the Service); render.sh: `iam-and-gateway-httproute` | as expected |
| M18 | names.sh: `short release`, `40-char release`, `52-char release`, `53-char release` (2 HTTPRoute objects share the name paigasus-console); render.sh: `iam-and-gateway-httproute` | as expected |
| M19 | ingress.sh: `route, annotations`, `route, no annotations`, `route, default timeouts`, `route, chart timeouts merge per key`, `route, empty zone key keeps the chart key`, `route, timeouts and annotations keys absent`; render.sh: `iam-and-gateway-httproute` | as expected |
| M20 | ingress.sh: `route, chart timeouts merge per key` (H6: gateway got `request: 30s`) | as expected |
| M21 | ingress.sh: `route, annotations`, `route, no annotations`, `route, default timeouts` (iam got `{}`), `route, no timeouts set` (H7), `route, timeouts and annotations keys absent` | as expected. The mutation renders `timeouts: {}` through `default "{}"`. |
| M22 | ingress.sh: `route, annotations`, `route, no annotations`, `route, default timeouts`, `route, chart timeouts merge per key`, `route, timeouts and annotations keys absent`; render.sh: `iam-and-gateway-httproute` | as expected. The golden row also went red: the third golden holds the `10m` default. |
| M23 | ingress.sh: `route, annotations` (H8); render.sh: `iam-and-gateway-httproute` | as expected |
| M24 | ingress.sh: `route, no annotations` and every other `route_shape` row with no annotations set (got `{}`) | as expected |
| M25 | kind check deleted: refusals.sh `httpRoute annotations not a map` (refused by `range can't iterate over x`, not the chart message). String check deleted: refusals.sh `httpRoute annotation not a string` (rendered) | as expected |
| M26 | kind check deleted: refusals.sh `httpRoute timeouts not a map` (refused by `range can't iterate over 10s`). Unknown-key check deleted: `httpRoute timeouts unknown key` (rendered). Form check deleted: `httpRoute timeout bad form`, `httpRoute timeout a number`, `zone timeout bad form` (rendered) | as expected |
| M27 | refusals.sh: `httpRoute backendRequest longer than request` (R14), `httpRoute backendRequest 1s over request 900ms` (Review Focus 1), `chart backendRequest over the zone default` (Review Focus 2) | as expected |
| M28 | refusals.sh: `httpRoute request 0s with backendRequest` (R15) | as expected |
| M29 | refusals.sh: `zone timeout on a disabled zone is not checked` (R16) | as expected |
| M30 | refusals.sh: `httpRoute backendRequest 1s over request 900ms` (rendered) and `httpRoute backendRequest 59s999ms under request 1m` (refused) | as expected: both Review Focus 1 rows |

### Full gate graph (Step 4)

The full `ci-targets` command (`moon ci … --base origin/main --include-relations`) exited
`rc=0`. It ran 24 actions. The four selected tasks all passed: `repo:input-liveness`,
`repo:helm-render`, `repo:affected-smoke` and `repo:actionlint`. No task failed, so no gate
needed the bash-version or pipe-size exception.

Note: at this run `origin/main` was `1a45803f` (`chore: release (#306)`), one commit after this
branch's base `4051df5e` (#332). The branch is not rebased onto it. The `open-pr` stage must
rebase before the push.

### Final verification after the rebase (2026-09-28)

The branch was rebased onto `origin/main` `1a45803f` with no conflicts. The release commit
changes no file under `charts/`, `ci/` or `docs/ops/`. The full `ci-targets` command then
exited `rc=0` again: 24 actions, and the four selected tasks passed (`repo:helm-render` was a
cache hit, `repo:input-liveness`, `repo:affected-smoke`, `repo:actionlint`). The actionlint
preflight reported `pipe capacity 65536 bytes (floor 8192)`, so its local verdict is valid.
`/bin/bash ci/helm-render/run.sh` ran uncached and passed. The four chart scripts also passed
under `/opt/homebrew/bin/bash` 5.3.15 with `--set ingress.host=console.example.test`.
