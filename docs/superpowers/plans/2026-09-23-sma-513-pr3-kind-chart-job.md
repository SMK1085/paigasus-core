# SMA-513 PR 3 — kind job, IdP CA value and docs: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the optional chart value `oidc.caBundle`, a non-required `chart.yml` workflow that installs the chart into kind behind Traefik with a real Keycloak and proves AC 1–4 with Playwright, the `repo:helm-render` follow-through (floor, counts, fixtures, a new kind-values row), Dependabot coverage for the job's manifests, and the chart runbook and CLAUDE.md notes.

**Architecture:** One script, `ci/kind/run.sh`, holds every mode (`up`, `images`, `install a`, `specs a`, `upgrade b`, `specs b`, `diagnose`, `down`). `.github/workflows/chart.yml` only installs tools and calls the modes, one step each with its own `timeout-minutes`. The cluster is kind v0.31.0 with a Kubernetes 1.31.14 node, Traefik 3.7.13 (chart 41.6.0, checked by SHA-256) on host ports 80/443, a throwaway CA made with `openssl`, a CoreDNS `hosts` block, and Postgres, Redis and Keycloak as digest-pinned manifests in `ci/kind/manifests/`. The chart installs with `ci/kind/values/a.yaml`, then upgrades with `a.yaml` + `b.yaml`. The Playwright specs live in `ts/apps/iam-console/tests/cluster/` with their own config and two projects, `phase-a` and `phase-b`.

**Tech Stack:** bash (3.2.57 and 5.x), Helm 3.22.0 (proto), kind v0.31.0, kubectl v1.31.14, Traefik v3.7.13 / chart 41.6.0, Keycloak 26.4, Postgres 16, Redis 7.4, OpenSSL/LibreSSL, Python 3 (PyYAML), Playwright 1.63 (Chromium), Moon 2.5.3, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-09-23-sma-513-pr3-kind-chart-job-design.md` (revision 2, approved at Gate 1). Context: `docs/superpowers/specs/2026-09-19-sma-513-multi-zone-ingress-helm-design.md` and `docs/superpowers/specs/2026-09-22-sma-513-pr2b-helm-render-gate-design.md`.

---

## Decision B10 — the ingress controller (verified 2026-09-23)

Gate 1 approved a rule: verify the ingress-nginx retirement; if confirmed, use Traefik; else ingress-nginx.

- **Source:** Kubernetes blog, "Ingress NGINX Retirement: What You Need to Know", 2025-11-11,
  <https://www.kubernetes.io/blog/2025/11/11/ingress-nginx-retirement/>. Quote: "Best-effort
  maintenance will continue until March 2026. Afterward, there will be no further releases, no
  bugfixes, and no updates to resolve any security vulnerabilities that may be discovered."
- **Measured:** `gh api repos/kubernetes/ingress-nginx --jq '.archived'` → `true`. The GitHub page
  says "archived by the owner on Mar 24, 2026". The last controller release is `controller-v1.15.1`,
  2026-03-19 (`gh api repos/kubernetes/ingress-nginx/releases --jq '.[0:5][] | .tag_name + " " + .published_at'`).
- **Result: retirement confirmed → Traefik.** `gh api repos/traefik/traefik-helm-chart/releases/latest --jq .tag_name` → `v41.6.0` (2026-09-16), `appVersion: v3.7.13` (read from the pulled chart's `Chart.yaml`).
- **Measured consequences** (from the pulled chart and the Traefik docs):
  - The chart `.tgz` SHA-256 is `cd7254ea853da73bdb88edc896f079b88d43ffa0bfe699fdbf21081361eac365`
    (`helm pull traefik --repo https://traefik.github.io/charts --version 41.6.0` then `shasum -a 256`).
  - `docker buildx imagetools inspect docker.io/traefik:v3.7.13 --format '{{json .Manifest.Digest}}'`
    → `sha256:24841fe2de7304c149343d877d2923b4c8800a38ba015dea9174c23b20e344a0`.
  - `helm template` of the chart with `ci/kind/traefik-values.yaml` (Task 6) renders rc 0 with
    `image: docker.io/traefik@sha256:2484…44a0`, `hostPort: 80`, `hostPort: 443`, `type: ClusterIP`,
    an `IngressClass` named `traefik` with `is-default-class: "false"`, and
    `--entryPoints.websecure.http.tls=true`.
  - An unmatched request gets **404** with the body `404 page not found` (Traefik FAQ,
    <https://doc.traefik.io/traefik/getting-started/faq/>: "every time a request cannot be matched
    with a router the correct response code is a `404 Not found`"). R3 and the settle step assert
    the body too (Review Focus 3).
  - Traefik sends `X-Forwarded-Proto`/`-Host`/`-Port` to the backend and does not trust incoming
    ones by default (`forwardedHeaders.insecure: false` in the chart values).
  - The spec's nginx proxy-buffer hazard (§ 6.5) does not apply. The README records that it would
    apply if the job ever moved back to nginx.

## Other measured facts this plan depends on

| Fact | Command | Result |
| -- | -- | -- |
| kind-action release | `gh api repos/helm/kind-action/releases/latest --jq .tag_name`; `gh api repos/helm/kind-action/git/ref/tags/v1.15.0 --jq '.object'` | `v1.15.0`, lightweight tag, commit `06c1ae10762d3b9c1644e7fe69596ae519e015a2` |
| kind-action inputs | `gh api "repos/helm/kind-action/contents/action.yml?ref=06c1ae1…" --jq .content \| base64 -d` | `install_only` exists (line 57); default kind `v0.33.0`, default kubectl `v1.37.0` |
| Newest kind with a 1.31 node | `gh api repos/kubernetes-sigs/kind/releases/tags/<t> --jq .body` for v0.26.0…v0.33.0 | v0.31.0 (2025-12-18): `kindest/node:v1.31.14@sha256:6f86cf509dbb42767b6e79debc3f2c32e4ee01386f0489b3b2be24b0a55aac2b`. v0.32.0 and v0.33.0 ship no 1.31 image |
| kubectl v1.31.14 exists | `curl -sL https://dl.k8s.io/release/v1.31.14/bin/linux/amd64/kubectl.sha256` | HTTP 200, `8791ec7c…4bdb0` |
| Keycloak digest | `docker buildx imagetools inspect quay.io/keycloak/keycloak:26.4 --format '{{json .Manifest.Digest}}'` | `sha256:9409c59bdfb65dbffa20b11e6f18b8abb9281d480c7ca402f51ed3d5977e6007` (26.4 → 26.4.7 today) |
| Redis digest | same, `docker.io/library/redis:7.4-alpine` | `sha256:858f009f9709ce576febc734aa78b8f6d624b82571f9ddb6bda4377c833b3499` |
| Postgres and curl digests | `sed -n 50,51p ci/images/run.sh` | re-used verbatim |
| `images.yml` amd64 leg | `gh run view 35895630391 --json jobs` | job 12 min 59 s: reclaim 1.62, OCI build of both Rust images 7.95, both consoles build + smoke 2.02 |
| `ci.yml` step times | `gh run view 35895630361 --json jobs` | pnpm install 0.15 min (warm cache), Playwright install 0.53 min |
| Dependabot `docker` reads K8s YAML | dependabot-core `main` at `9743eeaa4fcb6d7b13ff5f87c850d23363c77ba6`, `docker/lib/dependabot/shared/shared_file_fetcher.rb` and `docker/lib/dependabot/docker/file_parser.rb` | see Task 11 |
| Golden files render the `ID tokens` comment | `grep -n "ID tokens" charts/paigasus/tests/golden/*.yaml` | `iam-only.yaml:149`, `iam-and-gateway.yaml:164` (Spec defect 1) |
| Four fixtures are whole-file copies of files this PR edits | `diff charts/paigasus/templates/<f> ci/helm-render/fixtures/<fx>/templates/<f>` | `security-context` and `template-only-diff` (console-deployment.yaml), **`slug-mirror` and `zones-omits-enabled` (`_helpers.tpl`)** (Spec defect 2) |

## Global Constraints

- Work only in `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-kind-chart-job`. Before the first commit, run `git branch --show-current` and confirm `feature/sma-513-kind-chart-job`.
- `SCRATCH=/private/tmp/claude-501/sma-513-pr3` is the scratch directory. Create it with `mkdir -p`. Never write helper scripts into the repository.
- The worktree sandbox refuses compound shell commands with computed values. Put a multi-command step into a script file under `$SCRATCH` and run it with `/bin/bash <file>`.
- Prefix every tool command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text;`.
- Every new source file starts with an SPDX header: `# SPDX-License-Identifier: Apache-2.0` (shell, YAML), `// SPDX-License-Identifier: Apache-2.0` (TS), `{{/* SPDX-License-Identifier: Apache-2.0 */}}` (chart templates), `<!-- SPDX-License-Identifier: Apache-2.0 -->` (README files under `ci/`). JSON cannot carry a comment: `ci/kind/realm/paigasus-realm.json` has none, like its model `ts/packages/paigasus-auth/tests/e2e/keycloak-realm.json`. `docs/ops/RUNBOOK-*.md` and `CLAUDE.md` files carry none, like their siblings.
- Every new shell script: `set -euo pipefail`, then `export PROTO_REPORTER=text` at the top (SMA-609 standing rule).
- bash 3.2 and bash 5: no `mapfile`, no `declare -A`, no here-string (`<<<`), no here-doc in a step you run on the development Mac (bash 5 feeds a small here-doc through a pipe too), and a `"${A[@]+"${A[@]}"}"` guard on every array that can be empty.
- No pipe into an early-exit reader (`grep -q`, `grep -m`, `head`, `awk … exit`). `ci/actionlint/run.sh` check 13 scans every tracked `*.sh` and every workflow. Use `grep -q` only on a FILE.
- Exit codes: 0 pass, 1 an assertion failed, 2 an infrastructure error. `helm_render.py` keeps 0/3/2.
- Commit messages: Conventional Commits. The allowed scopes (`ts/packages/commitlint-config/index.cjs:42`) are `rs py ts contracts ci docs deps release repo claude workspace`. There is no `chart` scope: chart work is `repo`, gate work is `ci`. Every commit ends with the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No `#NNN` and no `token: value` line in a body (memory: commitlint footer gotcha).
- **Add commits. Never amend, never force-push, never `--no-verify`.**
- **The golden files must not change**, with ONE exception that this plan makes on purpose: Task 3 changes one rendered comment line in each golden file (Spec defect 1). Every other task runs `charts/paigasus/tests/render.sh` and expects `== chart render OK ==`.
- Never name a file with a Windows reserved base name (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`). The names in this plan were checked.
- Never make a directory named `build/` (the root `.gitignore` bare `build/` rule silently ignores it).
- A mutation proof is made AFTER the commit and restored with `git checkout -- <file>` (memory: mutation restore discards the fix).

## Review Focus

Five input classes or failure modes that the spec implies but that no spec row exercises. Most likely first. Each has a pinning test added to the task that owns it.

1. **`oidc.caBundle` values of an unexpected shape.** `helm upgrade --reuse-values` from a release made before this PR has NO `oidc.caBundle` map, and `--set oidc.caBundle=null` deletes it: a plain `.Values.oidc.caBundle.existingConfigMap` is then a nil-pointer template error. `--set oidc.caBundle.version=2` gives an `int64`, and `sha256sum` refuses a non-string. The templates read the value through `dig` helpers that always return a string. **Pinned:** `ca-bundle.sh` rows `null-cabundle` and `version-numbers` (Task 2).
2. **A `key` with a sub-path** (`certs/idp.pem`). The `items[].path`, the mount and the env path must agree, or the pod starts and Node only warns. **Pinned:** `ca-bundle.sh` row `set-subpath-key` (Task 2).
3. **A 404 from the wrong server.** R3 and the settle step accept "404 on `/gateway/overview`". A 404 page from a still-running gateway console (a Next `not-found`) would pass as "no route". **Pinned:** R3 and `settle` also require the Traefik body `404 page not found` (Tasks 7 and 9).
4. **The CoreDNS `hosts` block not inserted.** The insertion anchors on the `kubernetes cluster.local` line of kind's default Corefile. If a kind release changes that line, `awk` inserts nothing, pods resolve the two names through the upstream resolver, and the failure shows only much later as a TLS or DNS error in the console log. **Pinned:** `coredns_hosts` counts the inserted `hosts {` lines and exits 2 unless the count is exactly 1 (Task 7).
5. **The settle step with an unchanged pod template.** If `helm upgrade b` does not change the iam-console pod template (for example a future chart that moves the zone map out of the `checksum/zonemap` annotation), the old pod-template hash is also the new one, and "wait until no old pod exists" never ends. **Pinned:** `upgrade_b` compares the old and new `pod-template-hash` sets and exits 1 with a named message when they intersect (Task 7).

## File Structure

| File | Create/Modify | Responsibility |
| -- | -- | -- |
| `charts/paigasus/values.yaml` | Modify | `oidc.caBundle.{existingConfigMap,key,version}` with defaults |
| `charts/paigasus/templates/_helpers.tpl` | Modify | `paigasus.idpCa*` helpers (nil-safe, string-typed) and the empty-key refusal |
| `charts/paigasus/templates/console-deployment.yaml` | Modify | CA volume with `items`, read-only mount, `NODE_EXTRA_CA_CERTS`, `checksum/idp-ca` |
| `charts/paigasus/templates/backend-deployment.yaml` | Modify | the same for IAM with `IAM_AUTHN__EXTRA_CA_BUNDLE_PATH`; "ID tokens" → "access tokens" |
| `charts/paigasus/tests/ca-bundle.sh` | Create | spec § 5.4 rows plus Review Focus 1 and 2 rows |
| `charts/paigasus/tests/refusals.sh` | Modify | the empty-key refusal row |
| `charts/paigasus/tests/golden/iam-only.yaml`, `iam-and-gateway.yaml` | Modify | ONE comment line each (Task 3 only) |
| `charts/paigasus/README.md` | Modify | the value, the test list, "all seven" |
| `ci/helm-render/fixtures/{slug-mirror,zones-omits-enabled}/templates/_helpers.tpl` | Modify | re-sync from the live file, keep the mutation |
| `ci/helm-render/fixtures/{security-context,template-only-diff}/templates/console-deployment.yaml` | Modify | re-sync from the live file, keep the mutation |
| `ci/helm-render/run.sh` | Modify | `CHART_SCRIPT_FLOOR=7` |
| `ci/helm-render/helm_render.py` | Modify | row `7 kind-values`, `EXPECTED_ROW_LABELS`, self-test rows, stale counts |
| `ci/helm-render/README.md` | Modify | stale counts, row 7, delete-the-feature record |
| `ci/affected-graph/ci_targets.py` | Modify | the floor pin (`:1407`) and `SELF_TASK_EXPECTED_GLOBS["helm-render"]` |
| `moon.yml` | Modify | `repo:helm-render` input `ci/kind/values/**/*`; stale "six scripts" comment |
| `ci/kind/values/a.yaml`, `ci/kind/values/b.yaml` | Create | the release values (phase A) and the overlay (phase B) |
| `ci/kind/cluster.yaml` | Create | one kind node, host ports 80 and 443 |
| `ci/kind/traefik-values.yaml` | Create | Traefik by digest, hostPort 80/443, ClusterIP, class `traefik` |
| `ci/kind/manifests/postgres.yaml`, `redis.yaml`, `keycloak.yaml`, `gateway-absent.yaml`, `idp-preflight.yaml` | Create | the dependencies, the endpoint-less gateway Service, the discovery preflight pod |
| `ci/kind/realm/paigasus-realm.json` | Create | realm `paigasus` with placeholders |
| `ci/kind/run.sh` | Create | every mode |
| `ci/kind/README.md` | Create | modes, exit codes, evidence, local run, hazards |
| `ts/apps/iam-console/tests/cluster/playwright.config.ts` | Create | Chromium, host resolver rules, explicit timeouts, two projects |
| `ts/apps/iam-console/tests/cluster/support/login.ts` | Create | Keycloak login helper, session cookie, redirect chain |
| `ts/apps/iam-console/tests/cluster/phase-a/sso.spec.ts` | Create | R1, R1-control |
| `ts/apps/iam-console/tests/cluster/phase-a/cross-zone.spec.ts` | Create | R2 |
| `ts/apps/iam-console/tests/cluster/phase-a/nav-degraded.spec.ts` | Create | R3-control |
| `ts/apps/iam-console/tests/cluster/phase-b/zone-disabled.spec.ts` | Create | R3 |
| `ts/apps/iam-console/moon.yml` | Modify | `!tests/cluster/**` on `test` and `test-e2e` |
| `ts/apps/gateway-console/moon.yml` | Modify | `!/ts/apps/iam-console/tests/cluster/**` on `test-e2e` |
| `.github/workflows/chart.yml` | Create | the kind job |
| `ci/workflow-credentials/workflow_credentials.py` | Modify | `"chart.yml"` first in `EXPECTED_PR_SUBJECTS` |
| `ci/CLAUDE.md` | Modify | "five" → "seven" |
| `.github/dependabot.yml` | Modify | `docker` entry for `/ci/kind/manifests` |
| `docs/ops/RUNBOOK-chart.md` | Create | the chart runbook |
| `charts/CLAUDE.md` | Create | chart memory |
| `ts/CLAUDE.md` | Modify | one link line |
| `CLAUDE.md` | Modify | one memory-map row |

---

### Task 1: The `oidc.caBundle` values, helpers and refusal

**Files:**
- Modify: `charts/paigasus/values.yaml:78-86` (the `oidc` block; insert after line 86)
- Modify: `charts/paigasus/templates/_helpers.tpl:121-124` (refusal before the closing `end` of `paigasus.validate`) and `:148` (append helpers at the end)
- Modify: `charts/paigasus/tests/refusals.sh:102-103` (insert a row after the `duplicate basePath` row)
- Modify: `ci/helm-render/fixtures/slug-mirror/templates/_helpers.tpl` (whole file), `ci/helm-render/fixtures/zones-omits-enabled/templates/_helpers.tpl` (whole file)

**Interfaces:**
- Produces: helpers `paigasus.idpCaConfigMap`, `paigasus.idpCaKey`, `paigasus.idpCaVersion`, `paigasus.idpCaMountPath` (each renders a string; empty when unset). Task 2 consumes them.
- Produces: refusal text `oidc.caBundle.key is empty while oidc.caBundle.existingConfigMap is set`.

- [ ] **Step 1: Branch check and scratch dir**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-kind-chart-job
git branch --show-current
mkdir -p /private/tmp/claude-501/sma-513-pr3
```

Expected: `feature/sma-513-kind-chart-job`.

- [ ] **Step 2: Write the failing refusal row**

In `charts/paigasus/tests/refusals.sh`, after the `duplicate basePath` row (ends at line 102, `--set zones.gateway.basePath=/iam`), insert:

```bash
# oidc.caBundle (SMA-513 PR 3, spec § 5.2). The pods mount ONE key of the ConfigMap through
# `items`, so an empty key would render a volume item with no key, which the API server refuses at
# apply time, long after `helm template` said yes. Refuse it at render time instead.
expect_fail "oidc.caBundle key empty" "oidc.caBundle.key is empty while oidc.caBundle.existingConfigMap is set" \
  --set oidc.caBundle.existingConfigMap=paigasus-idp-ca --set oidc.caBundle.key=""
```

- [ ] **Step 3: Run it, expect FAIL**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
bash charts/paigasus/tests/refusals.sh --set ingress.host=console.example.test; echo "rc=$?"
```

Expected: the line `FAIL [oidc.caBundle key empty]: rendered, expected a refusal`, every other row `ok`, and `rc=1`.

- [ ] **Step 4: Add the values**

In `charts/paigasus/values.yaml`, after line 86 (the last `secretVersion` comment line, `# the console Deployment's checksum/secret annotation.`), insert:

```yaml
  # NOT required. Trust for an IdP whose certificate chain ends at a private CA (SMA-513 PR 3).
  # existingConfigMap names a ConfigMap that holds PEM root certificates under `key`. When it is
  # set, every console pod and the IAM pod mount that ONE key read-only under /etc/paigasus/idp-ca/,
  # the consoles get NODE_EXTRA_CA_CERTS and IAM gets IAM_AUTHN__EXTRA_CA_BUNDLE_PATH. A missing
  # ConfigMap or key stops the pod at volume setup. The consoles trust the bundle for EVERY TLS
  # connection (Redis included); IAM trusts it only for JWKS fetches. Put only the IdP's roots in it.
  # A CA certificate is public, so this is a ConfigMap, not a Secret. See docs/ops/RUNBOOK-chart.md.
  caBundle:
    existingConfigMap: ""
    key: ca.crt
    # NOT required. Bump (any string) after the ConfigMap's contents change: Node and IAM read the
    # bundle once, at process start, so only a restart picks up a new CA. Same shape as
    # oidc.secretVersion; it feeds a checksum/idp-ca annotation on all three Deployments.
    version: ""
```

- [ ] **Step 5: Add the helpers and the refusal**

In `charts/paigasus/templates/_helpers.tpl`, replace lines 121-124:

```
{{- if not .Values.ingress.host -}}
{{- fail "ingress.host is required; it is the single origin every zone's cookie is scoped to" -}}
{{- end -}}
{{- end -}}
```

with:

```
{{- if not .Values.ingress.host -}}
{{- fail "ingress.host is required; it is the single origin every zone's cookie is scoped to" -}}
{{- end -}}
{{- if and (include "paigasus.idpCaConfigMap" .) (not (include "paigasus.idpCaKey" .)) -}}
{{- fail "oidc.caBundle.key is empty while oidc.caBundle.existingConfigMap is set: every pod mounts ONE key of that ConfigMap, so the key must name it (the default is ca.crt)" -}}
{{- end -}}
{{- end -}}
```

Append at the end of the file (after line 148, `{{- end -}}` of `paigasus.serviceMapJson`):

```

{{/*
oidc.caBundle (SMA-513 PR 3). Read through `dig`, never as .Values.oidc.caBundle.<key>: a release
made before this value existed has no caBundle map under `helm upgrade --reuse-values`, and
`--set oidc.caBundle=null` deletes it, so the plain path is a nil-pointer template error. `include`
always yields a STRING, so a numeric `--set oidc.caBundle.version=2` still hashes.
*/}}
{{- define "paigasus.idpCaConfigMap" -}}
{{- dig "caBundle" "existingConfigMap" "" .Values.oidc -}}
{{- end -}}

{{- define "paigasus.idpCaKey" -}}
{{- dig "caBundle" "key" "" .Values.oidc -}}
{{- end -}}

{{- define "paigasus.idpCaVersion" -}}
{{- dig "caBundle" "version" "" .Values.oidc -}}
{{- end -}}

{{- define "paigasus.idpCaMountPath" -}}
/etc/paigasus/idp-ca
{{- end -}}
```

- [ ] **Step 6: Re-sync the two `_helpers.tpl` fixtures in THIS commit**

Why now and not in Task 4: Task 2's templates call `include "paigasus.idpCaConfigMap"`. A fixture chart built from a stale `_helpers.tpl` has no such template, every render in that fixture fails, and the negative control reports `INCONCLUSIVE` (rc 2). Keep each fixture a whole-file copy with its one mutation (`ci/helm-render/README.md:85-87`).

```bash
cp charts/paigasus/templates/_helpers.tpl ci/helm-render/fixtures/slug-mirror/templates/_helpers.tpl
cp charts/paigasus/templates/_helpers.tpl ci/helm-render/fixtures/zones-omits-enabled/templates/_helpers.tpl
```

Re-apply the `slug-mirror` mutation with Edit in `ci/helm-render/fixtures/slug-mirror/templates/_helpers.tpl` (line 9): replace the line `iam gateway` with `iam gateway billing`.

Re-apply the `zones-omits-enabled` mutation with Edit in `ci/helm-render/fixtures/zones-omits-enabled/templates/_helpers.tpl`: replace

```
{{- define "paigasus.zoneMapJson" -}}
{{- $m := dict -}}
{{- range $id, $z := .Values.zones -}}
{{- if $z.enabled -}}{{- $_ := set $m $id $z.basePath -}}{{- end -}}
```

with

```
{{- define "paigasus.zoneMapJson" -}}
{{- /* NEGATIVE-CONTROL FIXTURE (ci/helm-render): skips an ENABLED gateway zone. */ -}}
{{- $m := dict -}}
{{- range $id, $z := .Values.zones -}}
{{- if and $z.enabled (ne $id "gateway") -}}{{- $_ := set $m $id $z.basePath -}}{{- end -}}
```

Verify each fixture differs from the live file by its mutation only:

```bash
diff charts/paigasus/templates/_helpers.tpl ci/helm-render/fixtures/slug-mirror/templates/_helpers.tpl
diff charts/paigasus/templates/_helpers.tpl ci/helm-render/fixtures/zones-omits-enabled/templates/_helpers.tpl
```

Expected: first `9c9 / < iam gateway / --- / > iam gateway billing`; second `126a127` (the comment) and `129c130` (the `and … (ne $id "gateway")` line), as before this PR.

- [ ] **Step 7: Run the tests, expect PASS; prove the golden files and the gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
bash charts/paigasus/tests/refusals.sh --set ingress.host=console.example.test; echo "rc=$?"
bash charts/paigasus/tests/render.sh; echo "rc=$?"
bash ci/helm-render/run.sh --negative-control; echo "rc=$?"
bash ci/helm-render/run.sh; echo "rc=$?"
```

Expected: `ok [oidc.caBundle key empty]: refused with its own message` and `== chart refusals OK ==` rc 0; `== chart render OK ==` rc 0 (goldens unchanged, `git diff --stat charts/paigasus/tests/golden` empty); `== helm-render negative control passed (6 fixtures) ==` rc 0; `== helm-render: all checks passed ==` rc 0.

- [ ] **Step 8: Commit**

```bash
git add charts/paigasus/values.yaml charts/paigasus/templates/_helpers.tpl charts/paigasus/tests/refusals.sh \
  ci/helm-render/fixtures/slug-mirror/templates/_helpers.tpl ci/helm-render/fixtures/zones-omits-enabled/templates/_helpers.tpl
git commit -m "feat(repo): add the oidc.caBundle chart value and its empty-key refusal (SMA-513)

The value names a ConfigMap of PEM roots for an IdP with a private CA. It is
read through nil-safe dig helpers. The two _helpers.tpl negative-control
fixtures are re-synced in the same commit, because Task 2's templates include
the new helpers and a stale fixture would fail to render.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Render the CA bundle in the three Deployments, and `ca-bundle.sh`

**Files:**
- Create: `charts/paigasus/tests/ca-bundle.sh`
- Modify: `charts/paigasus/templates/console-deployment.yaml:32` (annotation), `:70` (env), `:90` (mount and volume)
- Modify: `charts/paigasus/templates/backend-deployment.yaml:47` (annotation), `:103` (env), `:120` (mount and volume)
- Modify: `ci/helm-render/fixtures/security-context/templates/console-deployment.yaml`, `ci/helm-render/fixtures/template-only-diff/templates/console-deployment.yaml` (whole files)
- Modify: `charts/paigasus/README.md:112-131` and a new section before `## The golden files` (line 97)

**Interfaces:**
- Consumes: the Task 1 helpers.
- Produces: in every console pod and the IAM pod: volume `idp-ca` (`configMap.name`, `items: [{key, path: key}]`, no `optional`), mount `idp-ca` at `/etc/paigasus/idp-ca`, `readOnly: true`; env `NODE_EXTRA_CA_CERTS` (consoles) or `IAM_AUTHN__EXTRA_CA_BUNDLE_PATH` (IAM) = `/etc/paigasus/idp-ca/<key>`; annotation `checksum/idp-ca` only when `version` is set. Task 5's `a.yaml` and the kind job consume this.

- [ ] **Step 1: Write the failing test script**

Create `charts/paigasus/tests/ca-bundle.sh` (mode 0755):

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# oidc.caBundle (SMA-513 PR 3, spec § 5). The value lets every pod trust an IdP whose chain ends at
# a private CA. One row per property:
#   set, set-custom-key, set-subpath-key
#                   each of the three Deployments mounts ONE key of the ConfigMap read-only through
#                   `items` (so a missing key stops the pod at volume setup, before Node can start
#                   with only a warning), gets its own env key in the POD env, and no version
#                   annotation while version is unset.
#   unset, null-cabundle
#                   nothing of it renders. render.sh's golden files pin the bytes; this row names
#                   the property. `null-cabundle` is `helm upgrade --reuse-values` from a release
#                   made before the value existed (Review Focus 1).
#   version-strings, version-numbers
#                   a version change restarts all three pods (every spec.template differs), and a
#                   NUMERIC version (an int64 from --set) still renders (Review Focus 1).
# Every row runs after a failure. python3 must have PyYAML, as for env.sh; repo:helm-render puts
# its venv first on PATH. Use only DOUBLE quotes inside the python3 blocks.
set -euo pipefail
export PROTO_REPORTER=text
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHART="$(cd "$HERE/.." && pwd)"
# The eighth copy of the required values (ci/helm-render/README.md, residual risk 4). The gateway
# zone is ON, so each row sees all three Deployments.
BASE=(
  --kube-version 1.31.0
  --set ingress.host=console.example.test
  --set ingress.tlsSecretName=console-tls
  --set oidc.issuer=https://idp.example.test/realms/paigasus
  --set oidc.clientId=paigasus-console
  --set oidc.existingSecret=paigasus-console-secret
  --set postgres.existingSecret=paigasus-postgres-secret
  --set zones.iam.backend.apiKeysPepperSecret=paigasus-iam-pepper
  --set zones.gateway.backend.url=http://gw.example.test:8088
  --set zones.gateway.enabled=true
  "$@"
)
ec=0

# The "${X[@]+...}" guards below are load-bearing, not noise. MEASURED: bash 3.2.57
# treats "${A[@]}" on an EMPTY array as an unbound variable under `set -u`, so a
# no-argument run would abort before the first row. bash 5.x does not. Some gates in
# this repo run under 3.2.

# Renders go to files, not through a pipe: a Linux runner holds the whole render in its pipe, a
# 512-byte host pipe does not (ci/helm-render/README.md, residual risk 5).
TMP="$(mktemp -d "${TMPDIR:-/tmp}/ca-bundle.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

# render <name> <helm args...>: writes $TMP/<name>.yaml. On failure it reports, sets ec=1 and
# returns 1, and the caller skips the rest of its row.
render() {
  local name="$1"; shift
  if ! helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" "$@" >"$TMP/$name.yaml" 2>"$TMP/$name.err"; then
    echo "FAIL [$name]: render failed"; cat "$TMP/$name.err"; ec=1; return 1
  fi
}

# verdict <label> <set|unset> <key> <render file>
verdict() {
  local label="$1" mode="$2" key="$3" file="$4" out
  if ! out="$(MODE="$mode" KEY="$key" python3 -c '
import os, sys, yaml
mode, key = os.environ["MODE"], os.environ["KEY"]
mount = "/etc/paigasus/idp-ca"
with open(sys.argv[1]) as fh:
    raw = fh.read()
docs = [d for d in yaml.safe_load_all(raw) if d]
deps = {}
for d in docs:
    if d.get("kind") == "Deployment":
        deps[d["spec"]["template"]["metadata"]["labels"]["app.kubernetes.io/name"]] = d
env_of = {"iam-console": "NODE_EXTRA_CA_CERTS", "gateway-console": "NODE_EXTRA_CA_CERTS", "iam-backend": "IAM_AUTHN__EXTRA_CA_BUNDLE_PATH"}
problems = []
if sorted(deps) != sorted(env_of):
    problems.append("Deployments are [" + ", ".join(sorted(deps)) + "], want [" + ", ".join(sorted(env_of)) + "]")
for name in sorted(env_of):
    d = deps.get(name)
    if d is None:
        continue
    tpl = d["spec"]["template"]
    pod = tpl["spec"]
    c = pod["containers"][0]
    vols = [v for v in pod.get("volumes") or [] if v.get("name") == "idp-ca"]
    mounts = [m for m in c.get("volumeMounts") or [] if m.get("name") == "idp-ca"]
    envs = [e for e in c.get("env") or [] if e.get("name") in ("NODE_EXTRA_CA_CERTS", "IAM_AUTHN__EXTRA_CA_BUNDLE_PATH")]
    if mode == "set":
        want_vol = [{"name": "idp-ca", "configMap": {"name": "paigasus-idp-ca", "items": [{"key": key, "path": key}]}}]
        want_mount = [{"name": "idp-ca", "mountPath": mount, "readOnly": True}]
        want_env = [{"name": env_of[name], "value": mount + "/" + key}]
        if vols != want_vol:
            problems.append(name + ": volume is " + repr(vols) + ", want " + repr(want_vol))
        if mounts != want_mount:
            problems.append(name + ": volumeMount is " + repr(mounts) + ", want " + repr(want_mount))
        if envs != want_env:
            problems.append(name + ": CA env is " + repr(envs) + ", want " + repr(want_env))
    elif vols or mounts or envs:
        problems.append(name + ": renders " + repr(vols + mounts + envs) + " with oidc.caBundle unset")
    if "checksum/idp-ca" in (tpl["metadata"].get("annotations") or {}):
        problems.append(name + ": checksum/idp-ca renders, but oidc.caBundle.version is unset")
for d in docs:
    if d.get("kind") == "ConfigMap" and "NODE_EXTRA_CA_CERTS" in (d.get("data") or {}):
        problems.append(d["metadata"]["name"] + " carries NODE_EXTRA_CA_CERTS; it belongs in the pod env")
if mode == "unset":
    for needle in ("idp-ca", "NODE_EXTRA_CA_CERTS", "EXTRA_CA_BUNDLE"):
        if needle in raw:
            problems.append("the render contains " + needle)
print("|".join(problems) if problems else "OK")' "$file" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

# differ <file 1> <file 2>: all three pod templates must differ (the check-3 style).
differ() {
  python3 -c '
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
problems += [n + ": spec.template is equal; it must differ" for n in want if n in a and n in b and a[n] == b[n]]
for n in want:
    if n in b and "checksum/idp-ca" not in (b[n]["metadata"].get("annotations") or {}):
        problems.append(n + ": no checksum/idp-ca annotation while oidc.caBundle.version is set")
print("|".join(problems) if problems else "OK")' "$1" "$2"
}

row_set() {  # row_set <label> <key> <helm args...>
  local label="$1" key="$2"; shift 2
  render "$label" "$@" || return 0
  verdict "$label" set "$key" "$TMP/$label.yaml"
}

row_unset() {  # row_unset <label> [helm args...]
  local label="$1"; shift
  render "$label" "$@" || return 0
  verdict "$label" unset ca.crt "$TMP/$label.yaml"
}

row_version() {  # row_version <label> <version 1> <version 2>
  local label="$1" v1="$2" v2="$3" out
  render "$label-1" --set oidc.caBundle.existingConfigMap=paigasus-idp-ca --set "oidc.caBundle.version=$v1" || return 0
  render "$label-2" --set oidc.caBundle.existingConfigMap=paigasus-idp-ca --set "oidc.caBundle.version=$v2" || return 0
  if ! out="$(differ "$TMP/$label-1.yaml" "$TMP/$label-2.yaml" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

row_set "set" ca.crt --set oidc.caBundle.existingConfigMap=paigasus-idp-ca
row_set "set-custom-key" bundle.pem --set oidc.caBundle.existingConfigMap=paigasus-idp-ca --set oidc.caBundle.key=bundle.pem
row_set "set-subpath-key" certs/idp.pem --set oidc.caBundle.existingConfigMap=paigasus-idp-ca --set oidc.caBundle.key=certs/idp.pem
row_unset "unset"
row_unset "null-cabundle" --set oidc.caBundle=null
row_version "version-strings" v1 v2
row_version "version-numbers" 1 2

if [ "$ec" -eq 0 ]; then echo "== chart ca-bundle OK =="; fi
exit "$ec"
```

```bash
chmod 0755 charts/paigasus/tests/ca-bundle.sh
```

- [ ] **Step 2: Run it, expect FAIL**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
PY="$(uv run --locked --project ci/helm-render python3 -c 'import sys; print(sys.executable)')"; echo "$PY"
```

Then with that interpreter's directory first on `PATH` (write this into `$SCRATCH/ca-bundle-run.sh` and run it with `/bin/bash`):

```bash
#!/bin/bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-kind-chart-job
PY="$(uv run --locked --project ci/helm-render python3 -c 'import sys; print(sys.executable)')"
PATH="$(dirname "$PY"):$PATH" "${BASH_UNDER_TEST:-/bin/bash}" charts/paigasus/tests/ca-bundle.sh --set ingress.host=console.example.test
echo "rc=$?"
```

Expected: `FAIL [set]`, `FAIL [set-custom-key]`, `FAIL [set-subpath-key]` (volume `[]`), `FAIL [version-strings]` and `FAIL [version-numbers]` (spec.template equal; no annotation); `ok [unset]` and `ok [null-cabundle]`; `rc=1`.

- [ ] **Step 3: Render the value in `console-deployment.yaml`**

After line 32 (`        checksum/secret: {{ $root.Values.oidc.secretVersion | sha256sum }}`), insert:

```
{{- if include "paigasus.idpCaVersion" $root }}
        # oidc.caBundle.version: the operator's signal that the CA ConfigMap changed. Node reads
        # NODE_EXTRA_CA_CERTS only at process start, so only a restart picks up a new CA.
        checksum/idp-ca: {{ include "paigasus.idpCaVersion" $root | sha256sum }}
{{- end }}
```

After line 70 (`                  key: session-redis-url`), insert:

```
{{- if include "paigasus.idpCaConfigMap" $root }}
            # oidc.caBundle (SMA-513 PR 3). Node reads this ONCE at start and then trusts the bundle
            # for EVERY TLS connection of the process, Redis included. In the pod env, not in the
            # shared console-env ConfigMap, so it appears only on a pod that also mounts the file.
            - name: NODE_EXTRA_CA_CERTS
              value: {{ printf "%s/%s" (include "paigasus.idpCaMountPath" $root) (include "paigasus.idpCaKey" $root) | quote }}
{{- end }}
```

After line 90 (`            failureThreshold: 3`, the last line before the two closing `{{- end }}`), insert:

```
{{- if include "paigasus.idpCaConfigMap" $root }}
          volumeMounts:
            - name: idp-ca
              mountPath: {{ include "paigasus.idpCaMountPath" $root }}
              readOnly: true
      # `items` and no `optional`: a missing ConfigMap or a missing key stops the pod at volume
      # setup, before Node starts. Without `items`, a missing key gives Node only a warning.
      volumes:
        - name: idp-ca
          configMap:
            name: {{ include "paigasus.idpCaConfigMap" $root | quote }}
            items:
              - key: {{ include "paigasus.idpCaKey" $root | quote }}
                path: {{ include "paigasus.idpCaKey" $root | quote }}
{{- end }}
```

Whitespace rule (golden files): every new block opens with `{{- if … }}` at column 0 and closes with `{{- end }}` at column 0, and every YAML comment is INSIDE the block. With the value unset the output is byte-identical.

- [ ] **Step 4: Render the value in `backend-deployment.yaml`**

After line 47 (`        checksum/pepper-secret: {{ $z.backend.apiKeysSecretVersion | sha256sum }}`), insert:

```
{{- if and (eq $id "iam") (include "paigasus.idpCaVersion" $root) }}
        # oidc.caBundle.version: IAM reads the bundle once at boot, so only a restart picks up a
        # new CA. Same shape as the checksums above.
        checksum/idp-ca: {{ include "paigasus.idpCaVersion" $root | sha256sum }}
{{- end }}
```

After line 103 (`                  key: pepper`), insert:

```
{{- if include "paigasus.idpCaConfigMap" $root }}
            # oidc.caBundle (SMA-513 PR 3): authn.extra_ca_bundle_path. IAM reads it once at boot and
            # uses it ONLY for JWKS fetches. A path with no PEM certificate is a boot failure.
            - name: IAM_AUTHN__EXTRA_CA_BUNDLE_PATH
              value: {{ printf "%s/%s" (include "paigasus.idpCaMountPath" $root) (include "paigasus.idpCaKey" $root) | quote }}
{{- end }}
```

After line 120 (`            failureThreshold: 3`, inside the `iam` branch, before `{{- else }}`), insert:

```
{{- if include "paigasus.idpCaConfigMap" $root }}
          volumeMounts:
            - name: idp-ca
              mountPath: {{ include "paigasus.idpCaMountPath" $root }}
              readOnly: true
      volumes:
        - name: idp-ca
          configMap:
            name: {{ include "paigasus.idpCaConfigMap" $root | quote }}
            items:
              - key: {{ include "paigasus.idpCaKey" $root | quote }}
                path: {{ include "paigasus.idpCaKey" $root | quote }}
{{- end }}
```

- [ ] **Step 5: Re-sync the two `console-deployment.yaml` fixtures**

```bash
cp charts/paigasus/templates/console-deployment.yaml ci/helm-render/fixtures/security-context/templates/console-deployment.yaml
cp charts/paigasus/templates/console-deployment.yaml ci/helm-render/fixtures/template-only-diff/templates/console-deployment.yaml
```

Re-apply `security-context` with Edit: replace the line `        runAsNonRoot: true` (the pod-level one, the only occurrence) with `        # NEGATIVE-CONTROL FIXTURE (ci/helm-render): runAsNonRoot dropped from the console pod.`

Re-apply `template-only-diff` with Edit: replace

```
          image: "{{ $z.console.image.repository }}:{{ $z.console.image.tag | default $root.Chart.AppVersion }}"
```

with

```
          # NEGATIVE-CONTROL FIXTURE (ci/helm-render): every console reads zones.iam's tag.
          image: "{{ $z.console.image.repository }}:{{ $root.Values.zones.iam.console.image.tag | default $root.Chart.AppVersion }}"
```

```bash
diff charts/paigasus/templates/console-deployment.yaml ci/helm-render/fixtures/security-context/templates/console-deployment.yaml
diff charts/paigasus/templates/console-deployment.yaml ci/helm-render/fixtures/template-only-diff/templates/console-deployment.yaml
```

Expected: exactly one hunk each (`37c37`; `42c42,43`), as before this PR.

- [ ] **Step 6: Run everything, expect PASS, under both bashes**

```bash
BASH_UNDER_TEST=/bin/bash /bin/bash /private/tmp/claude-501/sma-513-pr3/ca-bundle-run.sh
BASH_UNDER_TEST=/opt/homebrew/bin/bash /bin/bash /private/tmp/claude-501/sma-513-pr3/ca-bundle-run.sh
```

Expected, both: seven `ok` rows (`set`, `set-custom-key`, `set-subpath-key`, `unset`, `null-cabundle`, `version-strings`, `version-numbers`), `== chart ca-bundle OK ==`, `rc=0`.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
bash charts/paigasus/tests/render.sh; echo "rc=$?"
git diff --stat -- charts/paigasus/tests/golden
bash ci/helm-render/run.sh --negative-control; echo "rc=$?"
bash ci/helm-render/run.sh; echo "rc=$?"
```

Expected: `== chart render OK ==` rc 0; empty `git diff --stat`; negative control `passed (6 fixtures)` rc 0; the real run prints `PASS  [6 ca-bundle.sh]` among the chart scripts and `== helm-render: all checks passed ==` rc 0.

- [ ] **Step 7: Update `charts/paigasus/README.md`**

Before `## The golden files` (line 97), insert:

```markdown
## Trusting an IdP with a private CA (`oidc.caBundle`)

Both consumers of the issuer refuse a non-`https` URL, so an IdP with a private CA needs its root
in the pods. Set `oidc.caBundle.existingConfigMap` to a ConfigMap that holds PEM root certificates
under `oidc.caBundle.key` (default `ca.crt`). Every console pod and the IAM pod then mount that one
key read-only under `/etc/paigasus/idp-ca/`. The consoles get `NODE_EXTRA_CA_CERTS`, in the pod
env and not in the shared `console-env` ConfigMap. IAM gets `IAM_AUTHN__EXTRA_CA_BUNDLE_PATH`.

- The volume uses `items`, so a missing ConfigMap or key stops the pod at volume setup.
- `existingConfigMap` with an empty `key` is refused at render time.
- Bump `oidc.caBundle.version` after the ConfigMap changes. Node and IAM read the bundle once, at
  process start. The version feeds a `checksum/idp-ca` annotation on all three Deployments.
- The consoles trust the bundle for every TLS connection, Redis included. IAM trusts it only for
  JWKS fetches. Put only the roots the IdP needs in it.

With the value empty, the render is byte-identical to a chart without it. See
`docs/ops/RUNBOOK-chart.md` for the failure modes.
```

Replace lines 117-122 (the six script lines in the `## Running the tests` code block) with:

```bash
charts/paigasus/tests/refusals.sh --set ingress.host=console.example.test
charts/paigasus/tests/maps.sh
charts/paigasus/tests/ingress.sh
charts/paigasus/tests/render.sh
charts/paigasus/tests/names.sh
charts/paigasus/tests/env.sh
charts/paigasus/tests/ca-bundle.sh
```

In line 127-128, replace ``render.sh`, `names.sh` and `env.sh` set every REQUIRED value themselves`` with ``render.sh`, `names.sh`, `env.sh` and `ca-bundle.sh` set every REQUIRED value themselves``. In line 130, replace `runs all six in CI` with `runs all seven in CI`.

- [ ] **Step 8: Commit**

```bash
git add charts/paigasus/templates/console-deployment.yaml charts/paigasus/templates/backend-deployment.yaml \
  charts/paigasus/tests/ca-bundle.sh charts/paigasus/README.md \
  ci/helm-render/fixtures/security-context/templates/console-deployment.yaml \
  ci/helm-render/fixtures/template-only-diff/templates/console-deployment.yaml
git commit -m "feat(repo): mount the IdP CA bundle in the console and IAM pods (SMA-513)

One ConfigMap key, read-only, through items, so a missing key stops the pod at
volume setup. NODE_EXTRA_CA_CERTS goes into the console pod env and
IAM_AUTHN__EXTRA_CA_BUNDLE_PATH into the IAM pod. ca-bundle.sh pins the set,
unset, null, sub-path and version shapes. The golden files do not change.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 9: Delete-the-feature proof (spec § 10), after the commit**

With Edit, delete the whole `{{- if include "paigasus.idpCaConfigMap" $root }} … {{- end }}` block that holds `volumeMounts:` and `volumes:` in `charts/paigasus/templates/console-deployment.yaml`. Run:

```bash
BASH_UNDER_TEST=/bin/bash /bin/bash /private/tmp/claude-501/sma-513-pr3/ca-bundle-run.sh
```

Expected: `FAIL [set]`, `FAIL [set-custom-key]`, `FAIL [set-subpath-key]`, each naming `iam-console` and `gateway-console` (`volume is [], want …` and `volumeMount is [], want …`), `rc=1`. Record the three lines for the PR body. Restore:

```bash
git checkout -- charts/paigasus/templates/console-deployment.yaml
git status --short
```

Expected: `git status --short` prints nothing.

---

### Task 3: "ID tokens" → "access tokens", and the one-line golden re-baseline

This is its own commit so a reviewer can accept it or reject it alone. See Spec defect 1.

**Files:**
- Modify: `charts/paigasus/templates/backend-deployment.yaml:94`
- Modify: `charts/paigasus/tests/golden/iam-only.yaml:149`, `charts/paigasus/tests/golden/iam-and-gateway.yaml:164` (through `render.sh --update`)

**Interfaces:** none.

- [ ] **Step 1: Failing test first**

Edit `charts/paigasus/templates/backend-deployment.yaml` line 94: replace `            # the ID tokens IAM validates were issued to.` with `            # the access tokens IAM validates were issued to.` Then:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
bash charts/paigasus/tests/render.sh; echo "rc=$?"
```

Expected: two `diff -u` hunks, each `-            # the ID tokens IAM validates were issued to.` / `+            # the access tokens IAM validates were issued to.`, `FAIL [iam-only]`, `FAIL [iam-and-gateway]`, `rc=1`.

- [ ] **Step 2: Re-baseline deliberately and read the diff**

```bash
bash charts/paigasus/tests/render.sh --update
git diff --stat -- charts/paigasus/tests/golden
git diff -- charts/paigasus/tests/golden
bash charts/paigasus/tests/render.sh; echo "rc=$?"
```

Expected: `2 files changed, 2 insertions(+), 2 deletions(-)`; the diff holds only the comment line; then `== chart render OK ==` rc 0. If the diff holds anything else, stop: that is a defect in Task 1 or 2.

- [ ] **Step 3: Commit**

```bash
git add charts/paigasus/templates/backend-deployment.yaml charts/paigasus/tests/golden/iam-only.yaml charts/paigasus/tests/golden/iam-and-gateway.yaml
git commit -m "fix(repo): IAM validates access tokens, not ID tokens (SMA-513)

The comment renders into the manifest, so both golden files change by this one
line and by nothing else.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: `repo:helm-render` floor, stale counts, and checks 3 and 4 with the value set

**Files:**
- Modify: `ci/helm-render/run.sh:33`
- Modify: `ci/affected-graph/ci_targets.py:1407`
- Modify: `ci/helm-render/README.md:55`, `:122-123`
- Modify: `ci/helm-render/helm_render.py:52-53`
- Modify: `moon.yml:980-981`

**Interfaces:** Produces `CHART_SCRIPT_FLOOR=7`, pinned in both places.

- [ ] **Step 1: Failing test first — change one pin only**

Edit `ci/helm-render/run.sh:33`: `CHART_SCRIPT_FLOOR=6` → `CHART_SCRIPT_FLOOR=7`. Run the affected-graph self-check under system bash (it needs 3.2; root CLAUDE.md):

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
/bin/bash ci/affected-graph/run.sh; echo "rc=$?"
```

Expected: a row naming `ci/helm-render/run.sh: CHART_SCRIPT_FLOOR=6` as missing, and `rc=1`. (This is the F7 pin working.)

- [ ] **Step 2: Move the second pin**

Edit `ci/affected-graph/ci_targets.py:1407`: `    "CHART_SCRIPT_FLOOR=6",` → `    "CHART_SCRIPT_FLOOR=7",`.

- [ ] **Step 3: Fix the stale counts**

- `ci/helm-render/README.md:55`: `fewer than six chart scripts` → `fewer than seven chart scripts`.
- `ci/helm-render/README.md:122-123`: replace `4. \`STUB_VALUES\` is a seventh copy of the required values. A new required value must go into all` / `   seven; a missing one makes every render fail, which is rc 2.` with `4. \`STUB_VALUES\` is an eighth copy of the required values. A new required value must go into all` / `   eight, and into \`ci/kind/values/a.yaml\` (row 7); a missing one makes every render fail, which is rc 2.`
- `ci/helm-render/helm_render.py:52`: `# The required values, as ONE constant. A seventh copy of the list the six chart scripts hold` → `# The required values, as ONE constant. An eighth copy of the list the seven chart scripts hold`.
- `moon.yml:980-981`: replace `    # WHY THIS EXISTS — charts/paigasus/tests/ holds six scripts (lint, golden files, coupling,` / `    # maps, names, env, refusals) and until this task nothing ran them in CI. helm_render.py adds` with `    # WHY THIS EXISTS — charts/paigasus/tests/ holds seven scripts (render, ingress, maps, names,` / `    # env, refusals, ca-bundle) and until this task nothing ran them in CI. helm_render.py adds`.

- [ ] **Step 4: Run, expect PASS**

```bash
/bin/bash ci/affected-graph/run.sh --negative-control; echo "rc=$?"
/bin/bash ci/affected-graph/run.sh; echo "rc=$?"
bash ci/helm-render/run.sh --self-test; echo "rc=$?"
bash ci/helm-render/run.sh; echo "rc=$?"
```

Expected: both affected-graph runs rc 0; `== helm-render self-test passed ==`; `== helm-render: all checks passed ==` with seven `PASS  [6 …]` lines.

- [ ] **Step 5: Floor proof**

```bash
mv charts/paigasus/tests/ca-bundle.sh /private/tmp/claude-501/sma-513-pr3/ca-bundle.sh.moved
bash ci/helm-render/run.sh; echo "rc=$?"
mv /private/tmp/claude-501/sma-513-pr3/ca-bundle.sh.moved charts/paigasus/tests/ca-bundle.sh
git status --short
```

Expected: `FAIL  [6 floor]: 6 chart script(s) under …/charts/paigasus/tests, expected at least 7`, `rc=2`; afterwards `git status --short` shows only the four files edited in Steps 1–3 (and `moon.yml`).

- [ ] **Step 6: Checks 3 and 4 with the value set (spec § 5.5)**

Write `$SCRATCH/check34.py`:

```python
# Runs helm-render checks 3 and 4 with oidc.caBundle SET, which the gate's STUB_VALUES never set.
import sys
from pathlib import Path

sys.path.insert(0, "ci/helm-render")
import helm_render as hr  # noqa: E402

hr.STUB_VALUES = hr.STUB_VALUES + (
    ("oidc.caBundle.existingConfigMap", "paigasus-idp-ca"),
    ("oidc.caBundle.version", "v1"),
)
chart = Path("charts/paigasus").resolve()
rows = list(hr.check3(chart))
for label, enabled in hr.SUBSETS:
    rows += hr.check4(label, hr.parse_docs(hr.helm_template(chart, enabled)))
sys.exit(hr.report(rows))
```

And `$SCRATCH/check34.sh`:

```bash
#!/bin/bash
set -euo pipefail
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-kind-chart-job
PY="$(uv run --locked --project ci/helm-render python3 -c 'import sys; print(sys.executable)')"
HELM="$(proto --reporter text bin helm)"
PATH="$(dirname "$PY"):$(dirname "$HELM"):$PATH" "$PY" /private/tmp/claude-501/sma-513-pr3/check34.py
```

```bash
/bin/bash /private/tmp/claude-501/sma-513-pr3/check34.sh; echo "rc=$?"
```

Expected: `PASS` for `3a`, `3a-prime`, `3b`, `3c`, and for the eight `4 …` rows, then `== helm-render: all 12 rows passed ==`, `rc=0`. Record it in the PR body.

- [ ] **Step 7: Commit**

```bash
git add ci/helm-render/run.sh ci/affected-graph/ci_targets.py ci/helm-render/README.md ci/helm-render/helm_render.py moon.yml
git commit -m "feat(ci): raise the helm-render chart-script floor to seven (SMA-513)

run.sh and its ci_targets.py pin move in one commit. The stale six/seventh
counts in the README, helm_render.py and moon.yml are corrected.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: The kind values files and the required `7 kind-values` row

**Files:**
- Create: `ci/kind/values/a.yaml`, `ci/kind/values/b.yaml`
- Modify: `ci/helm-render/helm_render.py` (docstring `:3`, constants after `:47`, `EXPECTED_ROW_LABELS` `:87-108`, new `check7` before `# ---- run` near `:585`, `run_checks` `:636-637`, self-test `:937-938` and a new block before `# ---- row inventory floor`, argparse `:959`)
- Modify: `ci/helm-render/README.md` (Checks table after `:38`; delete-the-feature record `:89-95`)
- Modify: `moon.yml:1001-1008` and the INPUTS comment `:986-989`
- Modify: `ci/affected-graph/ci_targets.py:385-398`

**Interfaces:**
- Produces: `ci/kind/values/a.yaml` (Task 7 `install a`), `ci/kind/values/b.yaml` (Task 7 `upgrade b`); row `7 kind-values` in every real run.
- Consumes: the Task 2 value (`oidc.caBundle.existingConfigMap` in `a.yaml`), the Traefik class name `traefik` (Task 6).

- [ ] **Step 1: Write the values files**

`ci/kind/values/a.yaml`:

```yaml
# SPDX-License-Identifier: Apache-2.0
#
# The kind job's release values, phase A (SMA-513 PR 3, spec § 4.5): both zones on. The names
# below are the ones ci/kind/run.sh makes: the three Secrets, the CA ConfigMap, the TLS Secret, the
# endpoint-less gateway Service, and the three images it loads with `kind load` (tag `dev` is not
# `latest`, so the kubelet uses IfNotPresent and never pulls, decision B11).
#
# repo:helm-render row 7 renders this file (and this file plus b.yaml) on every PR. A new REQUIRED
# chart value must be set here too, or that required check goes red.
zones:
  iam:
    console:
      image:
        repository: iam-console
        tag: dev
      replicas: 1
    backend:
      image:
        repository: paigasus-iam
        tag: dev
      apiKeysPepperSecret: paigasus-iam-pepper
  gateway:
    enabled: true
    console:
      image:
        repository: gateway-console
        tag: dev
      replicas: 1
    backend:
      # A Service with no endpoints (ci/kind/manifests/gateway-absent.yaml). The probe gets a
      # refused connection, so the gateway zone is `degraded` in phase A (R3-control).
      url: http://gateway-absent.paigasus.svc.cluster.local:8088
ingress:
  host: console.paigasus.test
  className: traefik
  tlsSecretName: console-tls
oidc:
  issuer: https://idp.paigasus.test/realms/paigasus
  clientId: paigasus-console
  existingSecret: paigasus-oidc
  caBundle:
    existingConfigMap: paigasus-idp-ca
postgres:
  existingSecret: paigasus-postgres
```

`ci/kind/values/b.yaml`:

```yaml
# SPDX-License-Identifier: Apache-2.0
#
# Phase B overlay (spec § 4.5): `helm upgrade -f a.yaml -f b.yaml`. It holds only the change.
zones:
  gateway:
    enabled: false
```

- [ ] **Step 2: Write the failing self-test rows and the inventory entry**

In `ci/helm-render/helm_render.py`:

1. Append `"7 kind-values",` as the last element of `EXPECTED_ROW_LABELS` (after `"3c",`, line 107).
2. Line 937-938: `if len(EXPECTED_ROW_LABELS) != 20:` / `expected 20 labels` → `21` in both places.
3. Before the line `    # ---- row inventory floor (F1): …` (line 935), insert:

```python
    # ---- row 7 (kind values): the row follows helm's exit status; a missing file is rc 2
    class _Proc:
        def __init__(self, rc):
            self.returncode, self.stderr = rc, "stub stderr"

    with tempfile.TemporaryDirectory(prefix="helm-render-7-") as tmp:
        (Path(tmp) / "a.yaml").write_text("{}\n")
        (Path(tmp) / "b.yaml").write_text("{}\n")
        calls = []

        def ok_run(cmd, **_kw):
            calls.append(cmd)
            return _Proc(0)

        expect("check7 both renders exit 0", [check7(Path(tmp), run=ok_run, helm="helm-stub", values_dir=tmp)], passing=("7 kind-values",))
        if [c.count("-f") for c in calls] != [1, 2]:
            failures.append(f"check7: expected a render with a.yaml and one with a.yaml + b.yaml, got {calls}")
        expect("check7 a.yaml fails to render", [check7(Path(tmp), run=lambda cmd, **_kw: _Proc(1), helm="helm-stub", values_dir=tmp)], fail=("7 kind-values",))
        expect(
            "check7 only the overlay fails to render",
            [check7(Path(tmp), run=lambda cmd, **_kw: _Proc(1 if cmd.count("-f") == 2 else 0), helm="helm-stub", values_dir=tmp)],
            fail=("7 kind-values",),
        )
        (Path(tmp) / "b.yaml").unlink()
        expect_infra("check7 a missing b.yaml raises InfraError", lambda: check7(Path(tmp), run=ok_run, helm="helm-stub", values_dir=tmp))
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
bash ci/helm-render/run.sh --self-test; echo "rc=$?"
```

Expected: a Python `NameError: name 'check7' is not defined` from the module self-test, reported as `FAIL helm_render.py --self-test passes: expected 0, got 2`, `rc=2`.

- [ ] **Step 3: Implement `check7` and wire it**

After `KUBE_VERSION = "1.31.0"` block (after line 50, `SENTINEL_HOST = …`), add:

```python
# Row 7 (SMA-513 PR 3 spec § 5.5): the kind job's values files. The kind job is not a required
# check; this row is, so a chart change that breaks those values reds before merge.
KIND_VALUES = REPO_ROOT / "ci" / "kind" / "values"
```

Before the line `# --------------------------------------------------------------------------- run` (line 585), add:

```python
# --------------------------------------------------------------------------- check 7


def check7(chart, run=subprocess.run, helm=None, values_dir=KIND_VALUES):
    """Row 7: ci/kind/values/a.yaml, and a.yaml plus b.yaml, render against `chart` with rc 0.

    A render that fails is an ASSERTION (the row fails, rc 3): the chart and the kind values
    disagree, which is exactly what this row exists to catch. A missing values file is rc 2.
    `run`, `helm` and `values_dir` are parameters only so self_test() can drive the row without
    helm; production never passes them.
    """
    helm = helm or shutil.which("helm")
    if helm is None:
        raise InfraError("helm is not on PATH; run this module through ci/helm-render/run.sh")
    a, b = Path(values_dir) / "a.yaml", Path(values_dir) / "b.yaml"
    for f in (a, b):
        if not f.is_file():
            raise InfraError(f"the kind values file {f} does not exist")

    def body():
        problems = []
        for label, files in (("a.yaml", (a,)), ("a.yaml + b.yaml", (a, b))):
            cmd = [helm, "template", RELEASE, str(chart), "--kube-version", KUBE_VERSION]
            for f in files:
                cmd += ["-f", str(f)]
            proc = run(cmd, capture_output=True, text=True, check=False)
            if proc.returncode != 0:
                problems.append(f"{label}: helm template exited {proc.returncode}: {proc.stderr.strip()}")
        return problems

    return _row("7 kind-values", body)
```

In `run_checks` (line 636-637), replace

```python
    rows += check3(chart)
    _check_row_inventory([r.row for r in rows])
```

with

```python
    rows += check3(chart)
    rows.append(check7(chart))
    _check_row_inventory([r.row for r in rows])
```

Docstring line 3: `checks 1, 1a, 2, 3 and 4` → `checks 1, 1a, 2, 3, 4 and 7`. Argparse line 959: `"repo:helm-render checks 1, 1a, 2, 3 and 4"` → `"repo:helm-render checks 1, 1a, 2, 3, 4 and 7"`.

- [ ] **Step 4: The Moon input and its registry pin**

`moon.yml` repo:helm-render `inputs` (lines 1001-1008): after `      - 'ci/helm-render/**/*'` add `      - 'ci/kind/values/**/*'`. In the INPUTS comment (lines 986-989), after `…what check 1a reads.` add the sentence: `ci/kind/values/**/* is what row 7 renders; without it an edit to the kind values serves a cached PASS.`

`ci/affected-graph/ci_targets.py:385-398`: replace `    # SMA-513 PR 2b. Two globs then five literals,` with `    # SMA-513 PR 2b. Three globs then five literals,` and replace

```python
        "charts/**/*",
        "ci/helm-render/**/*",
        ".proto/plugins/helm.toml",
```

with

```python
        "charts/**/*",
        "ci/helm-render/**/*",
        # SMA-513 PR 3: row 7 renders the kind job's values files.
        "ci/kind/values/**/*",
        ".proto/plugins/helm.toml",
```

- [ ] **Step 5: README**

In `ci/helm-render/README.md`, after the `4 security-context` row of the Checks table (line 38), insert:

```markdown
| `7 kind-values` | The kind job's values render (SMA-513 PR 3) | `helm template` with `ci/kind/values/a.yaml`, or with `a.yaml` plus `b.yaml`, exits non-zero. A missing values file is rc 2 |
```

Append to the `## Delete-the-feature record` section (after line 95):

```markdown
Row 7 (SMA-513 PR 3): deleting `rows.append(check7(chart))` from `run_checks` makes the real run
exit 2 with `missing ['7 kind-values']`; deleting the `problems.append` in `check7`'s body makes
`--self-test` exit 2 with the two `check7 … fails to render` rows red.
```

- [ ] **Step 6: Run, expect PASS**

```bash
bash ci/helm-render/run.sh --self-test; echo "rc=$?"
bash ci/helm-render/run.sh --negative-control; echo "rc=$?"
bash ci/helm-render/run.sh; echo "rc=$?"
/bin/bash ci/affected-graph/run.sh; echo "rc=$?"
/opt/homebrew/bin/bash ci/ruff/run.sh; echo "rc=$?"
```

Expected: self-test passed rc 0; negative control passed (6 fixtures) rc 0 and each fixture reports `OK` (row 7 stays green in every fixture); the real run prints `PASS  [7 kind-values]` and `== helm-render: all 21 rows passed ==` before the chart scripts, rc 0; affected-graph rc 0 (the new input matches `SELF_TASK_EXPECTED_GLOBS`); ruff rc 0.

- [ ] **Step 7: Commit, then two delete-the-feature proofs**

```bash
git add ci/kind/values/a.yaml ci/kind/values/b.yaml ci/helm-render/helm_render.py ci/helm-render/README.md moon.yml ci/affected-graph/ci_targets.py
git commit -m "feat(ci): render the kind job's values in repo:helm-render (SMA-513)

Row 7 renders ci/kind/values/a.yaml, and a.yaml plus b.yaml, and requires rc 0.
A new required chart value that the kind values miss now reds a required check.
The Moon input and its SELF_TASK_EXPECTED_GLOBS entry move together.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

Proof A: delete the line `    rows.append(check7(chart))`; `bash ci/helm-render/run.sh; echo "rc=$?"` → `INFRA  helm-render: helm-render row inventory does not match EXPECTED_ROW_LABELS: missing ['7 kind-values']`, rc 2. `git checkout -- ci/helm-render/helm_render.py`.

Proof B: in `ci/kind/values/a.yaml` delete the line `  tlsSecretName: console-tls`; `bash ci/helm-render/run.sh; echo "rc=$?"` → `FAIL  [7 kind-values]: a.yaml: helm template exited 1: … ingress.tlsSecretName is required …`, rc 1. `git checkout -- ci/kind/values/a.yaml`. Record both in the PR body.

---

### Task 6: `ci/kind/` static files — cluster, Traefik values, manifests, realm

**Files:**
- Create: `ci/kind/cluster.yaml`, `ci/kind/traefik-values.yaml`
- Create: `ci/kind/manifests/postgres.yaml`, `redis.yaml`, `keycloak.yaml`, `gateway-absent.yaml`, `idp-preflight.yaml`
- Create: `ci/kind/realm/paigasus-realm.json`

**Interfaces:**
- Produces for Task 7: namespaces `paigasus` and `paigasus-deps` (made by `run.sh`); Services `postgres:5432`, `redis:6379`, `keycloak:8080` in `paigasus-deps`; Ingress `keycloak` on `idp.paigasus.test` with Secret `idp-tls`; Service `gateway-absent:8088` in `paigasus`; Pod `idp-preflight` in `paigasus`; the realm placeholders `__PAIGASUS_KIND_CLIENT_SECRET__` and `__PAIGASUS_KIND_USER_PASSWORD__`; the user `paigasus-kind`.
- Consumes: Secrets `postgres-auth` (key `password`) and ConfigMap `keycloak-realm` (key `paigasus-realm.json`) in `paigasus-deps`, ConfigMap `paigasus-idp-ca` in `paigasus` — all made by `run.sh up`.

Dependabot rule for every file in `ci/kind/manifests/` (Task 11): the FIRST YAML document must be a Kubernetes object with `apiVersion` and `kind`, and must not start with `---`. The Deployment comes first in each file.

- [ ] **Step 1: Failing check first**

Write `$SCRATCH/kind-static-check.py`:

```python
# Offline checks of ci/kind/'s static files: YAML/JSON parse, image pins, Dependabot's first-document rule.
import json
import re
import sys
from pathlib import Path

import yaml

root = Path("ci/kind")
problems = []
pinned = re.compile(r"^[a-z0-9./-]+(:[A-Za-z0-9._-]+)?@sha256:[0-9a-f]{64}$")
for f in sorted((root / "manifests").glob("*.yaml")) + [root / "cluster.yaml", root / "traefik-values.yaml"]:
    if not f.is_file():
        problems.append(f"{f}: missing")
        continue
    text = f.read_text()
    if not text.startswith("# SPDX-License-Identifier: Apache-2.0\n"):
        problems.append(f"{f}: no SPDX header")
    docs = [d for d in yaml.safe_load_all(text) if d]
    if f.parent.name == "manifests":
        if not (isinstance(docs[0], dict) and "apiVersion" in docs[0] and "kind" in docs[0]):
            problems.append(f"{f}: the first document is not a Kubernetes object (Dependabot reads only that one)")
        for line in text.splitlines():
            m = re.match(r"^\s+image:\s*(\S+)$", line)
            if m and not pinned.match(m.group(1)):
                problems.append(f"{f}: image {m.group(1)} is not pinned by digest")
want_files = {"postgres.yaml", "redis.yaml", "keycloak.yaml", "gateway-absent.yaml", "idp-preflight.yaml"}
got_files = {p.name for p in (root / "manifests").glob("*.yaml")}
if got_files != want_files:
    problems.append(f"manifests are {sorted(got_files)}, want {sorted(want_files)}")
realm_path = root / "realm" / "paigasus-realm.json"
realm = json.loads(realm_path.read_text()) if realm_path.is_file() else {}
text = realm_path.read_text() if realm_path.is_file() else ""
for ph in ("__PAIGASUS_KIND_CLIENT_SECRET__", "__PAIGASUS_KIND_USER_PASSWORD__"):
    if text.count(ph) != 1:
        problems.append(f"realm: placeholder {ph} occurs {text.count(ph)} times, want 1")
client = next((c for c in realm.get("clients", []) if c.get("clientId") == "paigasus-console"), {})
if client.get("redirectUris") != ["https://console.paigasus.test/iam/auth/callback", "https://console.paigasus.test/gateway/auth/callback"]:
    problems.append(f"realm: redirectUris are {client.get('redirectUris')}")
aud = [m for m in client.get("protocolMappers", []) if m.get("protocolMapper") == "oidc-audience-mapper"]
if not aud or aud[0]["config"].get("included.custom.audience") != "paigasus-console" or aud[0]["config"].get("access.token.claim") != "true":
    problems.append("realm: no audience mapper that puts paigasus-console into the access token")
for scope in ("basic", "profile", "email", "offline_access"):
    if scope not in client.get("defaultClientScopes", []):
        problems.append(f"realm: default client scope {scope} missing")
users = realm.get("users", [])
if len(users) != 1 or not users[0].get("email") or "offline_access" not in users[0].get("realmRoles", []):
    problems.append("realm: want one user with an email and the offline_access role")
print("\n".join(problems) if problems else "OK")
sys.exit(1 if problems else 0)
```

Write `$SCRATCH/kind-static-check.sh`:

```bash
#!/bin/bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-kind-chart-job
PY="$(uv run --locked --project ci/helm-render python3 -c 'import sys; print(sys.executable)')"
"$PY" /private/tmp/claude-501/sma-513-pr3/kind-static-check.py
echo "rc=$?"
```

```bash
/bin/bash /private/tmp/claude-501/sma-513-pr3/kind-static-check.sh
```

Expected: `… missing` lines for every file, `rc=1`.

- [ ] **Step 2: `ci/kind/cluster.yaml`**

```yaml
# SPDX-License-Identifier: Apache-2.0
#
# One kind node (SMA-513 PR 3, spec § 4.2 step 1). Host ports 80 and 443 map to the node, and
# Traefik binds them with hostPort (ci/kind/traefik-values.yaml): the recipe kind documents for an
# ingress controller. 127.0.0.1 only; Chromium reaches both hosts through --host-resolver-rules.
# The node image is not here: run.sh passes it with --image, pinned by digest.
kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
nodes:
  - role: control-plane
    extraPortMappings:
      - containerPort: 80
        hostPort: 80
        listenAddress: "127.0.0.1"
        protocol: TCP
      - containerPort: 443
        hostPort: 443
        listenAddress: "127.0.0.1"
        protocol: TCP
```

- [ ] **Step 3: `ci/kind/traefik-values.yaml`**

```yaml
# SPDX-License-Identifier: Apache-2.0
#
# Traefik for the kind job — decision B10. ingress-nginx is retired (Kubernetes blog, 2025-11-11;
# the repository was archived on 2026-03-24), so the job uses a maintained controller that serves
# plain networking.k8s.io/v1 Ingress. The chart renders an Ingress with no controller annotation,
# so nothing in charts/ names Traefik. run.sh installs chart 41.6.0 and checks its SHA-256 first.
#
# Measured with `helm template` of chart 41.6.0 and these values: the image renders as
# docker.io/traefik@sha256:…, both hostPorts render, the Service is ClusterIP, the IngressClass is
# `traefik` and not the default, and websecure has TLS on. An unmatched request gets 404 with the
# body `404 page not found` (R3 and the settle step depend on that body).
image:
  # Traefik v3.7.13 (the chart's appVersion). Refresh with:
  #   docker buildx imagetools inspect docker.io/traefik:v3.7.13 --format '{{json .Manifest.Digest}}'
  digest: sha256:24841fe2de7304c149343d877d2923b4c8800a38ba015dea9174c23b20e344a0
# With a digest the chart cannot read the version; it needs it for its version checks.
versionOverride: v3.7.13
ingressClass:
  enabled: true
  isDefaultClass: false
  name: traefik
ports:
  web:
    hostPort: 80
  websecure:
    hostPort: 443
service:
  spec:
    # The CoreDNS hosts block maps both names to this ClusterIP; kind has no load balancer.
    type: ClusterIP
updateStrategy:
  rollingUpdate:
    # A hostPort pod cannot surge on one node: the new pod would wait for the port forever.
    maxUnavailable: 1
    maxSurge: 0
```

- [ ] **Step 4: `ci/kind/manifests/postgres.yaml`**

```yaml
# SPDX-License-Identifier: Apache-2.0
#
# Postgres for IAM in the kind job (spec § 4.2 step 5). The image re-uses the digest pin of
# ci/images/run.sh:50. Dependabot (/ci/kind/manifests) can refresh this copy on its own; the two
# pins can then differ for a week, which is harmless. Refresh by hand with:
#   docker buildx imagetools inspect postgres:16-alpine --format '{{json .Manifest.Digest}}'
# The password is per run: run.sh makes the Secret postgres-auth.
apiVersion: apps/v1
kind: Deployment
metadata:
  name: postgres
  namespace: paigasus-deps
spec:
  replicas: 1
  selector:
    matchLabels:
      app.kubernetes.io/name: postgres
  template:
    metadata:
      labels:
        app.kubernetes.io/name: postgres
    spec:
      containers:
        - name: postgres
          image: postgres:16-alpine@sha256:cf78e76683b9ca8c5733cbbdce6c9262b45b6767934dd0a95e671f9a0fc20685
          env:
            - name: POSTGRES_USER
              value: paigasus
            - name: POSTGRES_DB
              value: iam
            - name: POSTGRES_PASSWORD
              valueFrom:
                secretKeyRef:
                  name: postgres-auth
                  key: password
          ports:
            - name: postgres
              containerPort: 5432
          readinessProbe:
            exec:
              command: ["pg_isready", "-U", "paigasus", "-d", "iam"]
            periodSeconds: 2
            failureThreshold: 60
---
apiVersion: v1
kind: Service
metadata:
  name: postgres
  namespace: paigasus-deps
spec:
  selector:
    app.kubernetes.io/name: postgres
  ports:
    - name: postgres
      port: 5432
      targetPort: postgres
```

- [ ] **Step 5: `ci/kind/manifests/redis.yaml`**

```yaml
# SPDX-License-Identifier: Apache-2.0
#
# Redis, the console session store in the kind job (spec § 4.2 step 5). No password: it is
# reachable only inside the cluster, and the cluster lives for one run. Refresh with:
#   docker buildx imagetools inspect redis:7.4-alpine --format '{{json .Manifest.Digest}}'
apiVersion: apps/v1
kind: Deployment
metadata:
  name: redis
  namespace: paigasus-deps
spec:
  replicas: 1
  selector:
    matchLabels:
      app.kubernetes.io/name: redis
  template:
    metadata:
      labels:
        app.kubernetes.io/name: redis
    spec:
      containers:
        - name: redis
          image: redis:7.4-alpine@sha256:858f009f9709ce576febc734aa78b8f6d624b82571f9ddb6bda4377c833b3499
          ports:
            - name: redis
              containerPort: 6379
          readinessProbe:
            exec:
              command: ["redis-cli", "ping"]
            periodSeconds: 2
            failureThreshold: 60
---
apiVersion: v1
kind: Service
metadata:
  name: redis
  namespace: paigasus-deps
spec:
  selector:
    app.kubernetes.io/name: redis
  ports:
    - name: redis
      port: 6379
      targetPort: redis
```

- [ ] **Step 6: `ci/kind/manifests/keycloak.yaml`**

```yaml
# SPDX-License-Identifier: Apache-2.0
#
# Keycloak for the kind job (spec § 4.2 step 5, § 4.3). TLS ends at Traefik; Keycloak serves plain
# HTTP on 8080 behind it (edge termination needs KC_HTTP_ENABLED). KC_HOSTNAME is a FULL URL
# (hostname v2), so the issuer is https://idp.paigasus.test/realms/paigasus for the browser AND for
# the pods, whatever Host header arrives. KC_PROXY_HEADERS=xforwarded reads the scheme Traefik sends.
# Health is on the management port 9000. The realm comes from the ConfigMap keycloak-realm, which
# run.sh writes per run with a generated client secret and user password; --import-realm reads
# /opt/keycloak/data/import/<realm>-realm.json. Docs: keycloak.org/server/hostname,
# /server/reverseproxy, /server/importExport, /observability/health. 26.4 is the tag the
# paigasus-auth e2e uses. Refresh with:
#   docker buildx imagetools inspect quay.io/keycloak/keycloak:26.4 --format '{{json .Manifest.Digest}}'
apiVersion: apps/v1
kind: Deployment
metadata:
  name: keycloak
  namespace: paigasus-deps
spec:
  replicas: 1
  selector:
    matchLabels:
      app.kubernetes.io/name: keycloak
  template:
    metadata:
      labels:
        app.kubernetes.io/name: keycloak
    spec:
      containers:
        - name: keycloak
          image: quay.io/keycloak/keycloak:26.4@sha256:9409c59bdfb65dbffa20b11e6f18b8abb9281d480c7ca402f51ed3d5977e6007
          args: ["start-dev", "--import-realm"]
          env:
            - name: KC_HOSTNAME
              value: https://idp.paigasus.test
            - name: KC_PROXY_HEADERS
              value: xforwarded
            - name: KC_HTTP_ENABLED
              value: "true"
            - name: KC_HEALTH_ENABLED
              value: "true"
          ports:
            - name: http
              containerPort: 8080
            - name: management
              containerPort: 9000
          readinessProbe:
            httpGet:
              path: /health/ready
              port: management
            periodSeconds: 5
            failureThreshold: 120
          volumeMounts:
            - name: realm
              mountPath: /opt/keycloak/data/import
              readOnly: true
      volumes:
        - name: realm
          configMap:
            name: keycloak-realm
---
apiVersion: v1
kind: Service
metadata:
  name: keycloak
  namespace: paigasus-deps
spec:
  selector:
    app.kubernetes.io/name: keycloak
  ports:
    - name: http
      port: 8080
      targetPort: http
---
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: keycloak
  namespace: paigasus-deps
spec:
  ingressClassName: traefik
  tls:
    - hosts:
        - idp.paigasus.test
      secretName: idp-tls
  rules:
    - host: idp.paigasus.test
      http:
        paths:
          - path: /
            pathType: Prefix
            backend:
              service:
                name: keycloak
                port:
                  name: http
```

- [ ] **Step 7: `ci/kind/manifests/gateway-absent.yaml`**

```yaml
# SPDX-License-Identifier: Apache-2.0
#
# zones.gateway.backend.url in ci/kind/values/a.yaml points here. The selector matches no pod, so
# the Service has no endpoints and a connection is refused at once: the gateway zone is
# `degraded` in phase A (spec § 4.5, R3-control). The chart does not deploy the gateway backend
# (parent spec D2), and the job does not need one (spec F2).
apiVersion: v1
kind: Service
metadata:
  name: gateway-absent
  namespace: paigasus
spec:
  selector:
    app.kubernetes.io/name: gateway-absent-no-such-pod
  ports:
    - name: http
      port: 8088
      targetPort: 8088
```

- [ ] **Step 8: `ci/kind/manifests/idp-preflight.yaml`**

```yaml
# SPDX-License-Identifier: Apache-2.0
#
# The discovery preflight (spec § 4.2 step 7). It fetches the discovery document through the
# in-cluster name, as IAM and the consoles do, and trusts ONLY the throwaway CA from the ConfigMap
# the chart mounts. run.sh reads its stdout and asserts `issuer` and an https `jwks_uri`. A wrong
# realm, CA or hostname option then fails here, named, and not later as an unusable IAM (spec F3).
# The image re-uses the digest pin of ci/images/run.sh:51. Refresh with:
#   docker buildx imagetools inspect curlimages/curl:8.11.1 --format '{{json .Manifest.Digest}}'
apiVersion: v1
kind: Pod
metadata:
  name: idp-preflight
  namespace: paigasus
spec:
  restartPolicy: Never
  containers:
    - name: curl
      image: curlimages/curl:8.11.1@sha256:c1fe1679c34d9784c1b0d1e5f62ac0a79fca01fb6377cdd33e90473c6f9f9a69
      command: ["curl"]
      args:
        - "-fsS"
        - "--retry"
        - "30"
        - "--retry-all-errors"
        - "--retry-delay"
        - "2"
        - "--max-time"
        - "10"
        - "--cacert"
        - "/etc/paigasus/idp-ca/ca.crt"
        - "https://idp.paigasus.test/realms/paigasus/.well-known/openid-configuration"
      volumeMounts:
        - name: idp-ca
          mountPath: /etc/paigasus/idp-ca
          readOnly: true
  volumes:
    - name: idp-ca
      configMap:
        name: paigasus-idp-ca
        items:
          - key: ca.crt
            path: ca.crt
```

- [ ] **Step 9: `ci/kind/realm/paigasus-realm.json`**

Modelled on `ts/packages/paigasus-auth/tests/e2e/keycloak-realm.json`. `basic` is in `defaultClientScopes` because in Keycloak 25+ the `sub` claim comes from that scope, and an explicit list replaces the realm default. `firstName`/`lastName` are set because Keycloak 26's user profile requires them, and a user without them gets an "update profile" form instead of a login.

```json
{
  "realm": "paigasus",
  "enabled": true,
  "sslRequired": "external",
  "clients": [
    {
      "clientId": "paigasus-console",
      "enabled": true,
      "protocol": "openid-connect",
      "publicClient": false,
      "clientAuthenticatorType": "client-secret",
      "secret": "__PAIGASUS_KIND_CLIENT_SECRET__",
      "standardFlowEnabled": true,
      "directAccessGrantsEnabled": false,
      "serviceAccountsEnabled": false,
      "implicitFlowEnabled": false,
      "redirectUris": [
        "https://console.paigasus.test/iam/auth/callback",
        "https://console.paigasus.test/gateway/auth/callback"
      ],
      "webOrigins": ["https://console.paigasus.test"],
      "attributes": {
        "post.logout.redirect.uris": "https://console.paigasus.test/iam/*##https://console.paigasus.test/gateway/*"
      },
      "protocolMappers": [
        {
          "name": "paigasus-console-audience",
          "protocol": "openid-connect",
          "protocolMapper": "oidc-audience-mapper",
          "consentRequired": false,
          "config": {
            "included.custom.audience": "paigasus-console",
            "id.token.claim": "false",
            "access.token.claim": "true",
            "introspection.token.claim": "true"
          }
        },
        {
          "name": "email",
          "protocol": "openid-connect",
          "protocolMapper": "oidc-usermodel-property-mapper",
          "consentRequired": false,
          "config": {
            "user.attribute": "email",
            "claim.name": "email",
            "jsonType.label": "String",
            "id.token.claim": "true",
            "access.token.claim": "true",
            "userinfo.token.claim": "true",
            "introspection.token.claim": "true"
          }
        }
      ],
      "defaultClientScopes": ["basic", "web-origins", "acr", "profile", "roles", "email", "offline_access"],
      "optionalClientScopes": ["address", "phone", "microprofile-jwt"]
    }
  ],
  "users": [
    {
      "username": "paigasus-kind",
      "enabled": true,
      "emailVerified": true,
      "email": "paigasus-kind@paigasus.test",
      "firstName": "Kind",
      "lastName": "Job",
      "realmRoles": ["offline_access"],
      "credentials": [
        {
          "type": "password",
          "value": "__PAIGASUS_KIND_USER_PASSWORD__",
          "temporary": false
        }
      ]
    }
  ]
}
```

- [ ] **Step 10: Run the check, expect PASS; render Traefik offline**

```bash
/bin/bash /private/tmp/claude-501/sma-513-pr3/kind-static-check.sh
```

Expected: `OK`, `rc=0`.

Write `$SCRATCH/traefik-render.sh`:

```bash
#!/bin/bash
set -euo pipefail
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
S=/private/tmp/claude-501/sma-513-pr3
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-kind-chart-job
export HELM_CACHE_HOME="$S/helm/cache" HELM_CONFIG_HOME="$S/helm/config" HELM_DATA_HOME="$S/helm/data"
rm -rf "$S/traefik"; mkdir -p "$S/traefik"
helm pull traefik --repo https://traefik.github.io/charts --version 41.6.0 --destination "$S/traefik"
shasum -a 256 "$S/traefik/traefik-41.6.0.tgz"
helm template traefik "$S/traefik/traefik-41.6.0.tgz" --namespace traefik -f ci/kind/traefik-values.yaml >"$S/traefik/out.yaml"
grep -c 'image: docker.io/traefik@sha256:24841fe2de7304c149343d877d2923b4c8800a38ba015dea9174c23b20e344a0' "$S/traefik/out.yaml"
grep -c -E 'hostPort: (80|443)$' "$S/traefik/out.yaml"
grep -c 'type: ClusterIP' "$S/traefik/out.yaml"
```

```bash
/bin/bash /private/tmp/claude-501/sma-513-pr3/traefik-render.sh
```

Expected: `cd7254ea853da73bdb88edc896f079b88d43ffa0bfe699fdbf21081361eac365  …/traefik-41.6.0.tgz`, then `1`, `2`, `1`.

- [ ] **Step 11: Commit**

```bash
git add ci/kind/cluster.yaml ci/kind/traefik-values.yaml ci/kind/manifests ci/kind/realm
git commit -m "feat(ci): kind cluster, Traefik values, dependency manifests and realm (SMA-513)

Traefik replaces the retired ingress-nginx (decision B10). Postgres, Redis,
Keycloak and the preflight curl are pinned by digest; the realm holds
placeholders only, filled per run by ci/kind/run.sh.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: `ci/kind/run.sh` — every mode

**Files:**
- Create: `ci/kind/run.sh` (mode 0755)

**Interfaces:**
- Consumes: Task 5 values, Task 6 files, `ci/images/run.sh build iam` (tags `paigasus-iam:dev`) and `ci/images/run.sh build-console` (tags `iam-console:dev`, `gateway-console:dev`) — `ci/images/run.sh:353,400`; the Task 9 Playwright config and its env `PAIGASUS_KIND_USERNAME`, `PAIGASUS_KIND_PASSWORD`, `PAIGASUS_KIND_OUTPUT_DIR`.
- Produces: state directory `$PAIGASUS_KIND_STATE` (default `${RUNNER_TEMP:-${TMPDIR:-/tmp}}/paigasus-kind`); the evidence directory `<state>/diagnose` that `chart.yml` uploads; exit codes 0/1/2.

- [ ] **Step 1: Measure the image load path (spec § 4.4) — first, with a stated fallback**

This measurement runs before the code exists. On the development Mac (best effort, Docker Desktop's containerd store) write `$SCRATCH/load-measure.sh`:

```bash
#!/bin/bash
set -u
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
S=/private/tmp/claude-501/sma-513-pr3
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-kind-chart-job
kind version
docker info --format '{{.Driver}} {{index .DriverStatus 0}}'
bash ci/images/run.sh build-console iam; echo "build rc=$?"
kind create cluster --name sma513-load --image kindest/node:v1.31.14@sha256:6f86cf509dbb42767b6e79debc3f2c32e4ee01386f0489b3b2be24b0a55aac2b --wait 120s; echo "create rc=$?"
kind load docker-image iam-console:dev --name sma513-load; echo "docker-image rc=$?"
docker save -o "$S/iam-console.tar" iam-console:dev; echo "save rc=$?"
kind load image-archive "$S/iam-console.tar" --name sma513-load; echo "image-archive rc=$?"
docker exec sma513-load-control-plane crictl images
kind delete cluster --name sma513-load
rm -f "$S/iam-console.tar"
```

```bash
/bin/bash /private/tmp/claude-501/sma-513-pr3/load-measure.sh
```

Record both rcs and the `crictl images` line for `docker.io/library/iam-console:dev` in the PR body. The CI reference is the first `chart.yml` run (Task 13). **Decision rule:** `run.sh` implements both paths behind `PAIGASUS_KIND_LOAD` (`docker-image`, the default, or `archive`). If the first CI run fails in `load_images` with the `docker-image` path, set the default to `archive` (the one-word edit in `load_images`) and push. Why a fallback exists: `kind load docker-image` runs `docker save` internally, which can fail on a multi-platform image in a containerd store (kind issue #3795); `docker save` of a single-platform image plus `kind load image-archive` avoids kind's own save.

- [ ] **Step 2: Failing test first**

```bash
bash ci/kind/run.sh nonsense; echo "rc=$?"
```

Expected: `bash: ci/kind/run.sh: No such file or directory`, `rc=127`.

- [ ] **Step 3: Write `ci/kind/run.sh`**

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# SMA-513 PR 3 — the kind job. One cluster, the chart installed the way an operator installs it,
# and the Playwright specs against a real ingress
# (docs/superpowers/specs/2026-09-23-sma-513-pr3-kind-chart-job-design.md).
#
#   run.sh up          kind cluster, Traefik, a throwaway CA and two leaves, the CoreDNS hosts
#                      block, Postgres, Redis, Keycloak, the three chart Secrets, the preflight
#   run.sh images      build paigasus-iam, iam-console and gateway-console; load them into kind
#   run.sh install a   helm install with values/a.yaml (both zones)
#   run.sh specs a     Playwright project phase-a (R1, R1-control, R2, R3-control)
#   run.sh upgrade b   helm upgrade with a.yaml + b.yaml (gateway zone off), then settle
#   run.sh specs b     Playwright project phase-b (R3), then R3's Deployment check
#   run.sh diagnose    evidence into <state>/diagnose (never a Secret, never the realm ConfigMap)
#   run.sh down        delete the cluster
#
# Exit codes: 0 pass | 1 a spec or an assertion failed (a failed helm install or upgrade counts:
# the chart is the unit under test) | 2 an infrastructure error.
#
# Runs under /bin/bash 3.2.57 and bash 5: no mapfile, no declare -A, no here-string, no pipe into
# an early-exit reader (ci/actionlint check 13). `images` calls ci/images/run.sh, which DOES use
# here-strings (its assert_pins), so that one mode can hang on a host whose new pipe holds 512
# bytes (root CLAUDE.md). Every wait has an explicit timeout.
set -euo pipefail

# proto prints NDJSON on stdout in an agent environment (SMA-609). Exported once, here.
export PROTO_REPORTER=text

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$REPO_ROOT/ci/kind"
STATE="${PAIGASUS_KIND_STATE:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/paigasus-kind}"
EVIDENCE="$STATE/diagnose"
CLUSTER=paigasus-kind
CONTEXT="kind-$CLUSTER"
RELEASE=paigasus
NS=paigasus
DEPS_NS=paigasus-deps
TRAEFIK_NS=traefik
CONSOLE_HOST=console.paigasus.test
IDP_HOST=idp.paigasus.test
ISSUER="https://$IDP_HOST/realms/paigasus"
KIND_USER=paigasus-kind

# kind v0.31.0 is the newest kind release with a Kubernetes 1.31 node image; the golden files are
# rendered for 1.31.0 (ci/helm-render/helm_render.py:47). chart.yml pins kind to v0.31.0 and
# kubectl to v1.31.14. Refresh: the "Images in this release" list of
#   https://github.com/kubernetes-sigs/kind/releases/tag/<kind version>
KIND_NODE_IMAGE="kindest/node:v1.31.14@sha256:6f86cf509dbb42767b6e79debc3f2c32e4ee01386f0489b3b2be24b0a55aac2b"

# Traefik, decision B10. The .tgz is checked before install. Refresh with:
#   helm pull traefik --repo https://traefik.github.io/charts --version <v>; shasum -a 256 traefik-<v>.tgz
TRAEFIK_CHART_REPO=https://traefik.github.io/charts
TRAEFIK_CHART_VERSION=41.6.0
TRAEFIK_CHART_SHA256=cd7254ea853da73bdb88edc896f079b88d43ffa0bfe699fdbf21081361eac365

# Traefik's own body for a request no router matches (Review Focus 3).
TRAEFIK_404_BODY="404 page not found"

USAGE="usage: ci/kind/run.sh up | images | install a | specs a|b | upgrade b | diagnose | down"

# helm's cache and config stay in the state directory, never in the operator's home.
export HELM_CACHE_HOME="$STATE/helm/cache" HELM_CONFIG_HOME="$STATE/helm/config" HELM_DATA_HOME="$STATE/helm/data"

die_infra() { printf 'kind: infrastructure error (rc=2): %s\n' "$*" >&2; exit 2; }
die_assert() { printf 'kind: assertion failed (rc=1): %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die_infra "$1 is not on PATH"; }
k() { kubectl --context "$CONTEXT" "$@"; }
h() { "$HELM_BIN" --kube-context "$CONTEXT" "$@"; }
sha256_of() {  # prints only the hex digest of $1
  local line
  if command -v sha256sum >/dev/null 2>&1; then line="$(sha256sum "$1")"; else line="$(shasum -a 256 "$1")"; fi
  printf '%s\n' "${line%% *}"
}

# helm through proto, pinned: the chart is rendered with the helm the golden files pin. Model:
# ci/helm-render/run.sh resolve_helm.
resolve_helm() {
  local want got bin
  want="$(sed -n 's/^helm = "\([0-9][0-9.]*\)"$/\1/p' "$REPO_ROOT/.prototools")" || die_infra "cannot read .prototools"
  case "$want" in ''|*[!0-9.]*) die_infra "expected one 'helm = \"X.Y.Z\"' pin in .prototools, got: ${want:-<none>}" ;; esac
  bin="$(cd "$REPO_ROOT" && proto --reporter text bin helm)" || die_infra "'proto --reporter text bin helm' failed; run 'proto install helm'"
  if [ ! -f "$bin" ] || [ ! -x "$bin" ]; then die_infra "helm did not resolve to an executable file. Got: ${bin:-<empty>}"; fi
  got="$("$bin" version --short)" || die_infra "'$bin version --short' failed"
  case "$got" in "v$want"|"v$want+"*) ;; *) die_infra "helm is $got but .prototools pins $want" ;; esac
  HELM_BIN="$bin"
}

# ---------------------------------------------------------------------------------------- up

install_traefik() {
  local dir="$STATE/traefik" got
  rm -rf "$dir"; mkdir -p "$dir"
  "$HELM_BIN" pull traefik --repo "$TRAEFIK_CHART_REPO" --version "$TRAEFIK_CHART_VERSION" --destination "$dir" \
    || die_infra "helm pull traefik $TRAEFIK_CHART_VERSION failed"
  got="$(sha256_of "$dir/traefik-$TRAEFIK_CHART_VERSION.tgz")" || die_infra "cannot hash the Traefik chart"
  [ "$got" = "$TRAEFIK_CHART_SHA256" ] \
    || die_infra "Traefik chart $TRAEFIK_CHART_VERSION has SHA-256 $got, want $TRAEFIK_CHART_SHA256"
  h install traefik "$dir/traefik-$TRAEFIK_CHART_VERSION.tgz" --namespace "$TRAEFIK_NS" --create-namespace \
    -f "$HERE/traefik-values.yaml" --wait --timeout 5m || die_infra "helm install traefik failed"
  TRAEFIK_IP="$(k -n "$TRAEFIK_NS" get service traefik -o jsonpath='{.spec.clusterIP}')" \
    || die_infra "cannot read the traefik Service"
  case "$TRAEFIK_IP" in ''|None) die_infra "the traefik Service has no ClusterIP" ;; esac
  echo "  traefik ClusterIP $TRAEFIK_IP"
}

# The leaf profile that IAM's rustls/webpki needs (spec § 4.2 step 3). Node and Chromium accept a
# v1 leaf, a leaf with no SAN and a CA used as a leaf; webpki refuses all three.
check_leaf() {
  local crt="$1" host="$2" text
  text="$(openssl x509 -in "$crt" -noout -text)" || die_infra "cannot read $crt"
  case "$text" in *"Version: 3 (0x2)"*) ;; *) die_infra "$crt is not X.509 v3" ;; esac
  case "$text" in *"DNS:$host"*) ;; *) die_infra "$crt has no subjectAltName DNS:$host" ;; esac
  case "$text" in *"TLS Web Server Authentication"*) ;; *) die_infra "$crt lacks extendedKeyUsage serverAuth" ;; esac
  case "$text" in *"CA:FALSE"*) ;; *) die_infra "$crt lacks basicConstraints CA:FALSE" ;; esac
  openssl verify -CAfile "$STATE/pki/ca.crt" "$crt" >/dev/null || die_infra "$crt does not verify against the throwaway CA"
}

make_pki() {
  local p="$STATE/pki" host
  rm -rf "$p"; mkdir -p "$p"
  printf '%s\n' '[req]' 'prompt = no' 'distinguished_name = dn' '[dn]' 'CN = paigasus-kind-throwaway-ca' \
    '[v3_ca]' 'basicConstraints = critical,CA:TRUE' 'keyUsage = critical,keyCertSign' 'subjectKeyIdentifier = hash' >"$p/ca.cnf"
  openssl req -x509 -new -nodes -newkey rsa:2048 -sha256 -days 2 -config "$p/ca.cnf" -extensions v3_ca \
    -keyout "$p/ca.key" -out "$p/ca.crt" 2>>"$p/openssl.log" || die_infra "openssl could not make the CA (see $p/openssl.log)"
  for host in "$CONSOLE_HOST" "$IDP_HOST"; do
    printf '%s\n' '[v3_leaf]' 'basicConstraints = CA:FALSE' 'extendedKeyUsage = serverAuth' "subjectAltName = DNS:$host" >"$p/$host.cnf"
    openssl req -new -nodes -newkey rsa:2048 -subj "/CN=$host" -keyout "$p/$host.key" -out "$p/$host.csr" 2>>"$p/openssl.log" \
      || die_infra "openssl could not make the $host key and CSR"
    openssl x509 -req -sha256 -days 2 -in "$p/$host.csr" -CA "$p/ca.crt" -CAkey "$p/ca.key" \
      -set_serial "0x$(openssl rand -hex 8)" -extfile "$p/$host.cnf" -extensions v3_leaf -out "$p/$host.crt" 2>>"$p/openssl.log" \
      || die_infra "openssl could not sign the $host leaf"
    check_leaf "$p/$host.crt" "$host"
  done
  echo "  CA and two leaves OK (v3, SAN, serverAuth, CA:FALSE, verified)"
}

# A `hosts` answer has the question name as its owner, so glibc accepts it (spec § 4.2 step 4).
coredns_hosts() {
  local f="$STATE/Corefile" n
  k -n kube-system get configmap coredns -o jsonpath='{.data.Corefile}' >"$f.orig" || die_infra "cannot read the CoreDNS Corefile"
  awk -v ip="$TRAEFIK_IP" -v a="$CONSOLE_HOST" -v b="$IDP_HOST" '
    /^[[:space:]]*kubernetes cluster\.local/ && !done {
      print "    hosts {"
      print "       " ip " " a
      print "       " ip " " b
      print "       fallthrough"
      print "    }"
      done = 1
    }
    { print }' "$f.orig" >"$f" || die_infra "awk could not edit the Corefile"
  n="$(grep -c '^    hosts {$' "$f")" || n=0
  [ "$n" = 1 ] || die_infra "inserted $n CoreDNS hosts block(s), want 1: the 'kubernetes cluster.local' anchor moved (see $f.orig)"
  k -n kube-system create configmap coredns --from-file=Corefile="$f" --dry-run=client -o yaml >"$f.yaml" \
    || die_infra "cannot build the CoreDNS ConfigMap"
  k apply -f "$f.yaml" >/dev/null || die_infra "cannot apply the CoreDNS ConfigMap"
  k -n kube-system rollout restart deployment coredns >/dev/null || die_infra "cannot restart CoreDNS"
  k -n kube-system rollout status deployment coredns --timeout=180s || die_infra "CoreDNS did not become ready"
}

make_credentials() {
  umask 077
  printf '%s' "$(openssl rand -hex 24)" >"$STATE/client-secret"
  printf '%s' "$(openssl rand -hex 16)" >"$STATE/user-password"
  printf '%s' "$(openssl rand -hex 24)" >"$STATE/postgres-password"
  printf '%s' "$(openssl rand -base64 32)" >"$STATE/pepper"
}

deps() {
  local realm="$STATE/paigasus-realm.json" d
  sed -e "s/__PAIGASUS_KIND_CLIENT_SECRET__/$(cat "$STATE/client-secret")/" \
      -e "s/__PAIGASUS_KIND_USER_PASSWORD__/$(cat "$STATE/user-password")/" \
      "$HERE/realm/paigasus-realm.json" >"$realm" || die_infra "cannot fill the realm"
  if grep -q '__PAIGASUS_KIND_' "$realm"; then die_infra "a realm placeholder survived in $realm"; fi
  k -n "$DEPS_NS" create secret generic postgres-auth --from-file=password="$STATE/postgres-password" >/dev/null \
    || die_infra "cannot create the Secret postgres-auth"
  k -n "$DEPS_NS" create configmap keycloak-realm --from-file=paigasus-realm.json="$realm" >/dev/null \
    || die_infra "cannot create the ConfigMap keycloak-realm"
  k apply -f "$HERE/manifests/postgres.yaml" -f "$HERE/manifests/redis.yaml" -f "$HERE/manifests/keycloak.yaml" \
    || die_infra "cannot apply the dependency manifests"
  for d in postgres redis keycloak; do
    k -n "$DEPS_NS" rollout status "deployment/$d" --timeout=600s || die_infra "$d did not become ready in 600 s"
  done
}

chart_secrets() {
  printf '%s' "postgres://paigasus:$(cat "$STATE/postgres-password")@postgres.$DEPS_NS.svc.cluster.local:5432/iam" >"$STATE/database-url"
  k -n "$NS" create secret generic paigasus-oidc \
    --from-file=oidc-client-secret="$STATE/client-secret" \
    --from-literal=session-redis-url="redis://redis.$DEPS_NS.svc.cluster.local:6379" >/dev/null \
    || die_infra "cannot create the Secret paigasus-oidc"
  k -n "$NS" create secret generic paigasus-postgres --from-file=database-url="$STATE/database-url" >/dev/null \
    || die_infra "cannot create the Secret paigasus-postgres"
  k -n "$NS" create secret generic paigasus-iam-pepper --from-file=pepper="$STATE/pepper" >/dev/null \
    || die_infra "cannot create the Secret paigasus-iam-pepper"
  k apply -f "$HERE/manifests/gateway-absent.yaml" >/dev/null || die_infra "cannot apply gateway-absent"
}

preflight() {
  local out="$EVIDENCE/idp-preflight.json"
  k -n "$NS" delete pod idp-preflight --ignore-not-found >/dev/null
  k apply -f "$HERE/manifests/idp-preflight.yaml" >/dev/null || die_infra "cannot start the preflight pod"
  if ! k -n "$NS" wait pod/idp-preflight --for=jsonpath='{.status.phase}'=Succeeded --timeout=180s; then
    k -n "$NS" logs pod/idp-preflight >"$out" 2>&1 || true
    k -n "$NS" describe pod/idp-preflight >"$EVIDENCE/idp-preflight.describe.txt" 2>&1 || true
    die_infra "the discovery preflight did not succeed; evidence in $out"
  fi
  k -n "$NS" logs pod/idp-preflight >"$out" || die_infra "cannot read the preflight output"
  ISSUER="$ISSUER" python3 -c '
import json, os, sys
with open(sys.argv[1]) as fh:
    doc = json.load(fh)
want = os.environ["ISSUER"]
problems = []
if doc.get("issuer") != want:
    problems.append("issuer is " + repr(doc.get("issuer")) + ", want " + repr(want))
if not str(doc.get("jwks_uri", "")).startswith("https://"):
    problems.append("jwks_uri is " + repr(doc.get("jwks_uri")) + ", want an https URL")
if problems:
    print("; ".join(problems))
    sys.exit(1)
print("  preflight OK: issuer " + want + ", jwks_uri " + doc["jwks_uri"])' "$out" \
    || die_infra "the discovery document is wrong; evidence in $out"
}

up() {
  need kind; need kubectl; need openssl; need python3; need docker; need curl
  resolve_helm
  mkdir -p "$STATE"; chmod 700 "$STATE"
  rm -rf "$EVIDENCE"; mkdir -p "$EVIDENCE"
  echo "== 1. kind cluster $CLUSTER ($KIND_NODE_IMAGE) =="
  kind create cluster --name "$CLUSTER" --image "$KIND_NODE_IMAGE" --config "$HERE/cluster.yaml" --wait 180s \
    || die_infra "kind create cluster failed (does a cluster named $CLUSTER exist already? run.sh down)"
  echo "== 2. Traefik $TRAEFIK_CHART_VERSION =="
  install_traefik
  echo "== 3. throwaway CA, two leaves, two TLS Secrets, the CA ConfigMap =="
  make_pki
  k create namespace "$NS" >/dev/null || die_infra "cannot create namespace $NS"
  k create namespace "$DEPS_NS" >/dev/null || die_infra "cannot create namespace $DEPS_NS"
  k -n "$NS" create secret tls console-tls --cert="$STATE/pki/$CONSOLE_HOST.crt" --key="$STATE/pki/$CONSOLE_HOST.key" >/dev/null \
    || die_infra "cannot create the Secret console-tls"
  k -n "$DEPS_NS" create secret tls idp-tls --cert="$STATE/pki/$IDP_HOST.crt" --key="$STATE/pki/$IDP_HOST.key" >/dev/null \
    || die_infra "cannot create the Secret idp-tls"
  k -n "$NS" create configmap paigasus-idp-ca --from-file=ca.crt="$STATE/pki/ca.crt" >/dev/null \
    || die_infra "cannot create the ConfigMap paigasus-idp-ca"
  echo "== 4. CoreDNS hosts block =="
  coredns_hosts
  echo "== 5. Postgres, Redis, Keycloak =="
  make_credentials
  deps
  echo "== 6. the three Secrets the chart refers to =="
  chart_secrets
  echo "== 7. discovery preflight =="
  preflight
  echo "== up: done =="
}

# ------------------------------------------------------------------------------------ images

load_images() {
  case "${PAIGASUS_KIND_LOAD:-docker-image}" in
    docker-image)
      kind load docker-image "$@" --name "$CLUSTER" || die_infra "kind load docker-image failed (try PAIGASUS_KIND_LOAD=archive)" ;;
    archive)
      docker save -o "$STATE/images.tar" "$@" || die_infra "docker save failed"
      kind load image-archive "$STATE/images.tar" --name "$CLUSTER" || die_infra "kind load image-archive failed"
      rm -f "$STATE/images.tar" ;;
    *) die_infra "PAIGASUS_KIND_LOAD must be docker-image or archive, got: $PAIGASUS_KIND_LOAD" ;;
  esac
  local img
  for img in "$@"; do
    docker exec "$CLUSTER-control-plane" crictl inspecti "docker.io/library/$img" >/dev/null \
      || die_infra "$img is not in the node's image store after the load"
    echo "  loaded $img"
  done
}

images() {
  need docker; need kind
  bash "$REPO_ROOT/ci/images/run.sh" build iam || die_infra "building paigasus-iam failed"
  bash "$REPO_ROOT/ci/images/run.sh" build-console || die_infra "building the two consoles failed"
  load_images paigasus-iam:dev iam-console:dev gateway-console:dev
}

# --------------------------------------------------------------------------- install/upgrade

install_a() {
  need kubectl; resolve_helm
  h install "$RELEASE" "$REPO_ROOT/charts/paigasus" --namespace "$NS" -f "$HERE/values/a.yaml" --wait --timeout 10m \
    || die_assert "helm install with values/a.yaml did not become ready in 10 minutes"
  echo "== install a: done =="
}

pod_hashes() {  # the pod-template-hash of every iam-console pod, space-separated
  k -n "$NS" get pods -l app.kubernetes.io/name=iam-console \
    -o jsonpath='{range .items[*]}{.metadata.labels.pod-template-hash}{" "}{end}'
}

# Spec § 4.5 settle step: no old iam-console pod, then Traefik's own 404 on /gateway/overview.
settle() {
  local old="$1" deadline left hash code body
  deadline=$(( $(date +%s) + 180 ))
  while :; do
    left=""
    for hash in $old; do
      left="$left$(k -n "$NS" get pods -l "app.kubernetes.io/name=iam-console,pod-template-hash=$hash" -o name)"
    done
    [ -z "$left" ] && break
    [ "$(date +%s)" -lt "$deadline" ] || die_infra "old iam-console pods still exist 180 s after the upgrade: $left"
    sleep 2
  done
  deadline=$(( $(date +%s) + 120 ))
  while :; do
    code="$(curl -sk -o "$STATE/settle-body" -w '%{http_code}' --resolve "$CONSOLE_HOST:443:127.0.0.1" \
      "https://$CONSOLE_HOST/gateway/overview" || true)"
    body="$(cat "$STATE/settle-body" 2>/dev/null || true)"
    if [ "$code" = 404 ] && [ "$body" = "$TRAEFIK_404_BODY" ]; then break; fi
    [ "$(date +%s)" -lt "$deadline" ] \
      || die_assert "/gateway/overview still answers $code ('$body') 120 s after the upgrade; want 404 '$TRAEFIK_404_BODY'"
    sleep 2
  done
  echo "  settled: no old iam-console pod; /gateway/overview answers Traefik's 404"
}

upgrade_b() {
  need kubectl; need curl; resolve_helm
  local old new h1 h2
  old="$(pod_hashes)" || die_infra "cannot list iam-console pods"
  [ -n "$old" ] || die_infra "no iam-console pod before the upgrade; run 'run.sh install a' first"
  h upgrade "$RELEASE" "$REPO_ROOT/charts/paigasus" --namespace "$NS" -f "$HERE/values/a.yaml" -f "$HERE/values/b.yaml" \
    --wait --timeout 10m || die_assert "helm upgrade with values/b.yaml did not become ready in 10 minutes"
  new="$(k -n "$NS" get replicasets -l app.kubernetes.io/name=iam-console \
    -o jsonpath='{range .items[?(@.spec.replicas>0)]}{.metadata.labels.pod-template-hash}{" "}{end}')" \
    || die_infra "cannot list iam-console ReplicaSets"
  # Review Focus 5: an unchanged pod template would make the wait below endless.
  for h1 in $old; do
    for h2 in $new; do
      [ "$h1" != "$h2" ] || die_assert "the iam-console pod template did not change on upgrade b (hash $h1): the zone map no longer rolls the console"
    done
  done
  settle "$old"
  echo "== upgrade b: done =="
}

# --------------------------------------------------------------------------------------- specs

specs() {
  local phase="$1" rc=0 out="$EVIDENCE/playwright/phase-$1" gw
  [ -f "$STATE/user-password" ] || die_infra "no credentials in $STATE; run 'run.sh up' first"
  need pnpm
  mkdir -p "$out"
  PAIGASUS_KIND_USERNAME="$KIND_USER" PAIGASUS_KIND_PASSWORD="$(cat "$STATE/user-password")" PAIGASUS_KIND_OUTPUT_DIR="$out" \
    pnpm --dir "$REPO_ROOT/ts/apps/iam-console" exec playwright test \
      --config tests/cluster/playwright.config.ts --project "phase-$phase" || rc=$?
  case "$rc" in
    0) ;;
    1) echo "FAIL: Playwright project phase-$phase" >&2 ;;
    *) die_infra "playwright exited $rc for phase-$phase" ;;
  esac
  if [ "$phase" = b ]; then
    # R3's last clause (spec § 6.4): the gateway console Deployment does not exist. Here and not in
    # the spec file, so the TS tier needs no child_process.
    gw="$(k -n "$NS" get deployments -l app.kubernetes.io/name=gateway-console -o name)" || die_infra "cannot list Deployments"
    if [ -n "$gw" ]; then echo "FAIL [R3]: the gateway console Deployment still exists: $gw" >&2; rc=1
    else echo "  ok [R3]: no gateway console Deployment"; fi
  fi
  return "$rc"
}

# ------------------------------------------------------------------------------------ diagnose

diagnose() {
  local ns p ready d="$EVIDENCE"
  mkdir -p "$d/logs" "$d/describe"
  k get all -A -o wide >"$d/get-all.txt" 2>&1 || true
  k get ingress,ingressclass -A -o wide >"$d/ingress.txt" 2>&1 || true
  k get events -A --sort-by=.lastTimestamp >"$d/events.txt" 2>&1 || true
  k -n kube-system get configmap coredns -o yaml >"$d/coredns.yaml" 2>&1 || true
  for ns in "$NS" "$DEPS_NS" "$TRAEFIK_NS"; do
    k -n "$ns" get pods -o jsonpath='{range .items[*]}{.metadata.name}{" "}{.status.conditions[?(@.type=="Ready")].status}{"\n"}{end}' \
      >"$d/pods-$ns.txt" 2>/dev/null || true
    while read -r p ready; do
      [ -n "$p" ] || continue
      k -n "$ns" logs --all-containers "$p" >"$d/logs/$ns-$p.log" 2>&1 || true
      k -n "$ns" logs --all-containers --previous "$p" >"$d/logs/$ns-$p.previous.log" 2>&1 || true
      if [ "$ready" != "True" ]; then k -n "$ns" describe pod "$p" >"$d/describe/$ns-$p.txt" 2>&1 || true; fi
    done <"$d/pods-$ns.txt"
  done
  if [ -n "${HELM_BIN:-}" ] || resolve_helm_quiet; then
    h -n "$NS" get manifest "$RELEASE" >"$d/helm-manifest.yaml" 2>&1 || true
  fi
  # EXCLUDED on purpose (spec § 4.6): Secrets, and the realm ConfigMap (it holds the client secret
  # and the password). `get all` lists neither. The Playwright traces under playwright/ hold the
  # per-run password as typed; it is a throwaway that dies with the cluster.
  echo "evidence in $d"
}

resolve_helm_quiet() {  # diagnose must not exit 2 because helm is missing
  local bin
  bin="$(cd "$REPO_ROOT" && proto --reporter text bin helm 2>/dev/null)" || return 1
  [ -x "$bin" ] || return 1
  HELM_BIN="$bin"
}

down() {
  need kind
  kind delete cluster --name "$CLUSTER" || die_infra "kind delete cluster failed"
}

cmd="${1:-}"
case "$cmd" in
  up) up ;;
  images) images ;;
  install) [ "${2:-}" = a ] || die_infra "$USAGE"; install_a ;;
  upgrade) [ "${2:-}" = b ] || die_infra "$USAGE"; upgrade_b ;;
  specs)
    case "${2:-}" in a|b) ;; *) die_infra "$USAGE" ;; esac
    s_rc=0; specs "$2" || s_rc=$?
    exit "$s_rc" ;;
  diagnose) diagnose ;;
  down) down ;;
  *) die_infra "$USAGE" ;;
esac
```

```bash
chmod 0755 ci/kind/run.sh
```

- [ ] **Step 4: Run the static checks, expect PASS**

```bash
bash ci/kind/run.sh nonsense; echo "rc=$?"
/bin/bash -n ci/kind/run.sh; echo "3.2 syntax rc=$?"
/opt/homebrew/bin/bash -n ci/kind/run.sh; echo "5 syntax rc=$?"
grep -n -E 'mapfile|declare -A|<<<' ci/kind/run.sh; echo "banned-constructs grep rc=$? (want 1)"
grep -n -E '\|[[:space:]]*(grep[[:space:]]+-[a-zA-Z]*[qm]|head([[:space:];)]|$)|awk[^|]*exit)' ci/kind/run.sh; echo "early-exit grep rc=$? (want 1)"
```

Expected: `kind: infrastructure error (rc=2): usage: ci/kind/run.sh up | images | …`, `rc=2`; both syntax checks rc 0; both greps rc 1 (no match). The authoritative check-13 verdict is the actionlint run in Task 13.

- [ ] **Step 5: Local bring-up, best effort (the development Mac)**

Only when Docker Desktop runs, kind v0.31.0 is on `PATH`, and host ports 80/443 are free. Run `bash ci/kind/run.sh up; echo "rc=$?"`, then `bash ci/kind/run.sh diagnose` and `bash ci/kind/run.sh down`. Expected on success: `  preflight OK: issuer https://idp.paigasus.test/realms/paigasus, jwks_uri https://idp.paigasus.test/realms/paigasus/protocol/openid-connect/certs` and `== up: done ==`, rc 0. A failure here is not a blocker: CI is the reference (spec § 9). Record what happened in the PR body.

- [ ] **Step 6: Commit**

```bash
git add ci/kind/run.sh
git commit -m "feat(ci): ci/kind/run.sh, the kind job's modes (SMA-513)

up, images, install a, specs a|b, upgrade b, diagnose and down. The preflight
asserts the discovery document through the in-cluster name; the settle step
waits for the old iam-console pods and for Traefik's own 404.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: `ci/kind/README.md`

**Files:**
- Create: `ci/kind/README.md`

**Interfaces:** none.

- [ ] **Step 1: Write the README**

```markdown
<!-- SPDX-License-Identifier: Apache-2.0 -->

# The kind job (`ci/kind/`)

`.github/workflows/chart.yml` installs `charts/paigasus` into a kind cluster behind Traefik, with a
real Keycloak, and runs the Playwright specs in `ts/apps/iam-console/tests/cluster/`. It is NOT a
required check. A broken chart reds `main`, not the pull request. `repo:helm-render` row 7 renders
`values/a.yaml` and `values/a.yaml` + `values/b.yaml` on every pull request, so a values break
reds a required check.

Spec: `docs/superpowers/specs/2026-09-23-sma-513-pr3-kind-chart-job-design.md`.

## Modes

| Command | Does |
| -- | -- |
| `bash ci/kind/run.sh up` | kind v0.31.0 with a Kubernetes 1.31.14 node; Traefik 3.7.13 (chart 41.6.0, SHA-256 checked); a throwaway CA and two leaves; the CoreDNS `hosts` block; Postgres, Redis and Keycloak; the three chart Secrets; the discovery preflight |
| `bash ci/kind/run.sh images` | `ci/images/run.sh build iam` and `build-console`, then `kind load` of `paigasus-iam:dev`, `iam-console:dev`, `gateway-console:dev` |
| `bash ci/kind/run.sh install a` | `helm install` with `values/a.yaml` (both zones, the CA bundle set) |
| `bash ci/kind/run.sh specs a` | Playwright project `phase-a`: R1, R1-control, R2, R3-control |
| `bash ci/kind/run.sh upgrade b` | `helm upgrade` with `a.yaml` + `b.yaml` (the gateway zone off), then the settle step |
| `bash ci/kind/run.sh specs b` | Playwright project `phase-b` (R3), then R3's Deployment check |
| `bash ci/kind/run.sh diagnose` | evidence into `<state>/diagnose` |
| `bash ci/kind/run.sh down` | deletes the cluster |

The state directory is `$PAIGASUS_KIND_STATE`, else `$RUNNER_TEMP/paigasus-kind`, else
`$TMPDIR/paigasus-kind`. It holds the per-run credentials; only its `diagnose/` subdirectory is
uploaded.

## Exit codes

| Code | Meaning |
| -- | -- |
| 0 | pass |
| 1 | a spec or an assertion failed. A failed `helm install` or `helm upgrade` counts: the chart is the unit under test |
| 2 | an infrastructure error: a tool, the cluster, a dependency, the preflight, a build or an image load |

## Reading the evidence

On a failure or a cancel, `chart.yml` runs `diagnose` and uploads `kind-evidence` (7 days).

- `get-all.txt`, `ingress.txt`, `events.txt`, `coredns.yaml`: the cluster state.
- `logs/<namespace>-<pod>.log` and `.previous.log`: every pod in `paigasus`, `paigasus-deps` and
  `traefik`. `describe/` holds each pod that is not Ready.
- `helm-manifest.yaml`: what the chart rendered.
- `idp-preflight.json`: the discovery document the pods saw.
- `playwright/phase-a|b/`: the HTML report and the traces of failed tests.

Secrets and the realm ConfigMap are never collected. The Playwright traces hold the per-run user
password as typed. It is a throwaway that is valid only while that one cluster exists.

Where to look first:

- The login ends on the IAM console, but IAM shows as unusable: IAM refused the token. Read
  `logs/paigasus-*-iam-backend-*.log` for the JWKS fetch or the `aud` check (spec F3, F4).
- The preflight fails: read `idp-preflight.json` and the Keycloak log. A wrong `issuer` means the
  Keycloak hostname options; a TLS error means the CA or the CoreDNS block.

## Hazards

- **Keycloak tabs (SMA-652).** A Keycloak page must not be re-used or closed after a login. The
  login helper never does either.
- **nginx proxy buffers.** Keycloak's large `Set-Cookie` headers can exceed an nginx proxy buffer and
  give a 502 at login. This job uses Traefik, so it does not apply. If the job ever moves to an
  nginx controller, set `proxy-buffer-size` on the Keycloak Ingress.
- **Here-strings in `images`.** `ci/images/run.sh` uses here-strings, so `run.sh images` can hang on
  a host whose new pipe holds 512 bytes (root `CLAUDE.md`). The other modes do not use them.
- **The CoreDNS `hosts` block is a kind-only device.** A real cluster needs real DNS for the IdP.

## Pins and their refresh

| Pin | Where | Refresh |
| -- | -- | -- |
| kind node image | `run.sh` `KIND_NODE_IMAGE` | the release notes of the kind version `chart.yml` pins |
| kind, kubectl | `chart.yml` (`helm/kind-action` inputs) | kind v0.31.0 is the newest release with a 1.31 node |
| Traefik chart | `run.sh` `TRAEFIK_CHART_VERSION`, `TRAEFIK_CHART_SHA256` | `helm pull traefik --repo https://traefik.github.io/charts --version <v>`; `shasum -a 256` |
| Traefik image | `traefik-values.yaml` | `docker buildx imagetools inspect docker.io/traefik:<v> --format '{{json .Manifest.Digest}}'` |
| Postgres, Redis, Keycloak, curl | `manifests/*.yaml` | Dependabot (`/ci/kind/manifests`); the command is in each file |

## Local run

Best effort only. Docker Desktop's containerd image store differs from the runner's classic store
(root memory: Docker Desktop store vs CI runner), so an image load can fail here and pass in CI. CI
is the reference. You need Docker, kind v0.31.0, kubectl, `proto install helm node pnpm`,
`pnpm --dir ts install`, the Chromium from
`pnpm --dir ts/apps/iam-console exec playwright install chromium`, and free host ports 80 and 443.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/kind/run.sh up
bash ci/kind/run.sh images        # set PAIGASUS_KIND_LOAD=archive if docker-image fails
bash ci/kind/run.sh install a
bash ci/kind/run.sh specs a
bash ci/kind/run.sh upgrade b
bash ci/kind/run.sh specs b
bash ci/kind/run.sh down
```
```

- [ ] **Step 2: Commit**

```bash
git add ci/kind/README.md
git commit -m "docs(ci): README for the kind job (SMA-513)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: The Playwright cluster specs and the Moon input exclusions

**Files:**
- Create: `ts/apps/iam-console/tests/cluster/playwright.config.ts`
- Create: `ts/apps/iam-console/tests/cluster/support/login.ts`
- Create: `ts/apps/iam-console/tests/cluster/phase-a/sso.spec.ts`, `cross-zone.spec.ts`, `nav-degraded.spec.ts`
- Create: `ts/apps/iam-console/tests/cluster/phase-b/zone-disabled.spec.ts`
- Modify: `ts/apps/iam-console/moon.yml:263` (after) and `:336` (after); `ts/apps/gateway-console/moon.yml:358` (after)

**Interfaces:**
- Consumes: `waitForHydration` from `ts/apps/iam-console/tests/e2e/support/hydration.ts:42`; env `PAIGASUS_KIND_USERNAME`, `PAIGASUS_KIND_PASSWORD`, `PAIGASUS_KIND_OUTPUT_DIR` (from `run.sh specs`).
- Produces: projects `phase-a` and `phase-b` for `run.sh specs a|b`.

**These specs cannot run locally without the cluster.** Their "failing test first" step is typecheck and lint of a deliberately broken import; their red/green proof is the CI run of `chart.yml` (Task 13), including the two delete-the-feature runs.

- [ ] **Step 1: Failing check first**

Create `ts/apps/iam-console/tests/cluster/phase-a/sso.spec.ts` with only:

```ts
// SPDX-License-Identifier: Apache-2.0
import { loginAt } from '../support/login';

void loginAt;
```

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
moon run iam-console-ts:typecheck; echo "rc=$?"
```

Expected: `error TS2307: Cannot find module '../support/login'`, a failed task, rc 1. This proves `tsc -p tsconfig.json` covers `tests/cluster/` (the app `tsconfig.json` includes `**/*.ts` and excludes only `node_modules` and `tests/fixtures/**`).

- [ ] **Step 2: `tests/cluster/playwright.config.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The cluster tier (SMA-513 PR 3, spec § 6): Playwright against the chart installed in kind behind
// Traefik, with a real Keycloak. Run it only through `ci/kind/run.sh specs a|b`, which selects a
// project and passes the per-run credentials. It lives INSIDE tests/ so the app's typecheck and
// lint inputs (`tests/**/*`) reach it; a config at the app root would get a cached PASS.
import os from 'node:os';
import path from 'node:path';
import { defineConfig, devices } from '@playwright/test';

// OUTSIDE the app directory: the app is Tailwind's scan root and Moon hashes it.
const outputRoot = process.env.PAIGASUS_KIND_OUTPUT_DIR ?? path.join(os.tmpdir(), 'iam-console-cluster-results');

export default defineConfig({
  testDir: '.',
  testMatch: '**/*.spec.ts',
  forbidOnly: !!process.env.CI,
  fullyParallel: false,
  workers: 1,
  // 0, on purpose: the kind job is the measurement (spec § 10), and a retry would hide a flake.
  retries: 0,
  reporter: [['list'], ['html', { outputFolder: path.join(outputRoot, 'report'), open: 'never' }]],
  outputDir: path.join(outputRoot, 'results'),
  // Every limit is explicit: in Playwright 1.63 actions, navigations and `waitFor` have NO
  // default limit (ts/CLAUDE.md), so an unbounded wait would eat the step's timeout.
  timeout: 120_000,
  expect: { timeout: 15_000 },
  use: {
    baseURL: 'https://console.paigasus.test',
    // The throwaway CA from ci/kind/run.sh up.
    ignoreHTTPSErrors: true,
    actionTimeout: 15_000,
    navigationTimeout: 30_000,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    // The job never edits /etc/hosts (decision B4). Only Chromium's own resolver sees these names,
    // so no spec may use Playwright's Node-side request API (APIRequestContext) against them.
    launchOptions: {
      args: ['--host-resolver-rules=MAP console.paigasus.test 127.0.0.1, MAP idp.paigasus.test 127.0.0.1'],
    },
  },
  projects: [
    { name: 'phase-a', testDir: './phase-a', use: { ...devices['Desktop Chrome'] } },
    { name: 'phase-b', testDir: './phase-b', use: { ...devices['Desktop Chrome'] } },
  ],
});
```

- [ ] **Step 3: `tests/cluster/support/login.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
import { expect, type Page, type Request, type Response } from '@playwright/test';
import { waitForHydration } from '../../e2e/support/hydration';

export { waitForHydration };

export const CONSOLE_HOST = 'console.paigasus.test';
export const IDP_HOST = 'idp.paigasus.test';
export const SESSION_COOKIE = '__Host-pgs_sid';
/** Traefik's body for a request no router matches (spec B10; Review Focus 3). */
export const TRAEFIK_404_BODY = '404 page not found';

function credential(name: 'PAIGASUS_KIND_USERNAME' | 'PAIGASUS_KIND_PASSWORD'): string {
  const value = process.env[name];
  if (value === undefined || value === '') {
    throw new Error(`${name} is not set; run these specs through ci/kind/run.sh specs a|b`);
  }
  return value;
}

/**
 * Open `path` with no session, fill Keycloak's form, and wait until the zone page is back and
 * hydrated (spec § 6.2). It never re-uses and never closes a Keycloak page (SMA-652): the page it
 * leaves behind is the console page, not the Keycloak one.
 */
export async function loginAt(page: Page, path: string): Promise<void> {
  await page.goto(path);
  await expect(page).toHaveURL((url) => url.hostname === IDP_HOST);
  await page.locator('#username').fill(credential('PAIGASUS_KIND_USERNAME'));
  await page.locator('#password').fill(credential('PAIGASUS_KIND_PASSWORD'));
  await page.locator('#kc-login').click();
  await page.waitForURL((url) => url.hostname === CONSOLE_HOST && url.pathname === path);
  await waitForHydration(page);
}

/** The value of the shared session cookie in this page's context. */
export async function sessionCookie(page: Page): Promise<string> {
  const cookie = (await page.context().cookies()).find((c) => c.name === SESSION_COOKIE);
  if (cookie === undefined) throw new Error(`no ${SESSION_COOKIE} cookie in this context`);
  return cookie.value;
}

/** Every hop of the navigation that produced `response`, first hop first. */
export async function redirectChain(response: Response): Promise<{ readonly url: URL; readonly status: number }[]> {
  const chain: { url: URL; status: number }[] = [];
  let request: Request | null = response.request();
  while (request !== null) {
    const hop = await request.response();
    chain.unshift({ url: new URL(request.url()), status: hop?.status() ?? 0 });
    request = request.redirectedFrom();
  }
  return chain;
}
```

- [ ] **Step 4: `phase-a/sso.spec.ts` (R1, R1-control)**

Replace the Step 1 stub with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// AC 4: one login serves both zones. R1 counts IdP requests because Keycloak keeps its own SSO
// cookie: a second console login would pass with no form, so "no form appeared" proves nothing.
// R1-control proves the gateway zone WOULD redirect without the session (spec § 6.3).
import { expect, test } from '@playwright/test';
import { CONSOLE_HOST, IDP_HOST, loginAt, redirectChain, sessionCookie, waitForHydration } from '../support/login';

test('R1: a login in the IAM zone admits the gateway zone with no IdP request (AC 4)', async ({ page, context }) => {
  await loginAt(page, '/iam/orgs');
  const before = await sessionCookie(page);

  const second = await context.newPage();
  const idpRequests: string[] = [];
  second.on('request', (request) => {
    const url = new URL(request.url());
    if (url.hostname === IDP_HOST) idpRequests.push(url.pathname);
  });
  const response = await second.goto('/gateway/overview');
  if (response === null) throw new Error('page.goto(/gateway/overview) returned no response');

  expect(response.request().redirectedFrom(), 'the document must arrive in one hop, with no redirect').toBeNull();
  expect(response.status()).toBe(200);
  expect(new URL(second.url()).pathname).toBe('/gateway/overview');
  expect(idpRequests, 'the gateway zone must not send the browser to the IdP').toEqual([]);
  expect(await sessionCookie(second)).toBe(before);
  await waitForHydration(second);
});

test('R1-control: without the session, the gateway zone redirects to its own login (AC 4)', async ({ page }) => {
  expect(await page.context().cookies()).toEqual([]);
  // A browser navigation, not `request.get`: the Node-side request API does not see Chromium's
  // --host-resolver-rules, so it cannot resolve console.paigasus.test (Spec defect 4).
  const response = await page.goto('/gateway/overview');
  if (response === null) throw new Error('page.goto(/gateway/overview) returned no response');
  const [first, second] = await redirectChain(response);
  expect(first?.url.pathname).toBe('/gateway/overview');
  expect([302, 303, 307]).toContain(first?.status ?? 0);
  expect(second?.url.hostname).toBe(CONSOLE_HOST);
  expect(second?.url.pathname).toBe('/gateway/auth/login');
});
```

- [ ] **Step 5: `phase-a/cross-zone.spec.ts` (R2)**

```ts
// SPDX-License-Identifier: Apache-2.0
//
// AC 1 and AC 3 through a real ingress (spec § 6.3 R2). The gateway zone's IAM link is a cross-zone
// <ZoneLink>, a plain <a> with no client router (spec F2), so the click must make a new DOCUMENT
// request; a same-zone link would not. "Your organizations" renders only when IAM accepted the
// access token. A hydrated page on both sides proves each zone's `_next` assets load under its own
// base path through Traefik (docs/ops/RUNBOOK-containers.md:370-374 left that proof open).
import { expect, test } from '@playwright/test';
import { CONSOLE_HOST, loginAt, waitForHydration } from '../support/login';

test('R2: the gateway zone links to IAM with a hard navigation, and both zones hydrate (AC 1, AC 3)', async ({ page }) => {
  await loginAt(page, '/gateway/overview');

  const documents: string[] = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (request.resourceType() === 'document' && url.hostname === CONSOLE_HOST) documents.push(url.pathname);
  });
  await page.getByRole('navigation', { name: 'Primary' }).getByRole('link', { name: 'IAM', exact: true }).click();
  await page.waitForURL((url) => url.pathname === '/iam/orgs');

  expect(documents, 'the IAM link must be a hard navigation (a new document request)').toContain('/iam/orgs');
  await expect(page.getByRole('heading', { level: 1, name: 'Your organizations' })).toBeVisible();
  await waitForHydration(page);
});
```

- [ ] **Step 6: `phase-a/nav-degraded.spec.ts` (R3-control)**

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The in-phase control for R3 (spec § 6.3): with the gateway zone ENABLED and its backend down
// (ci/kind/manifests/gateway-absent.yaml), the IAM nav shows the Gateway entry as degraded — a span
// with role="link" and aria-disabled (primary-nav.tsx:98). R3 uses the same locator in phase B.
import { expect, test } from '@playwright/test';
import { loginAt } from '../support/login';

test('R3-control: the IAM console shows the Gateway entry as degraded (AC 2)', async ({ page }) => {
  await loginAt(page, '/iam/orgs');
  const gateway = page.getByRole('navigation', { name: 'Primary' }).getByRole('link', { name: 'Gateway', exact: true });
  await expect(gateway).toHaveCount(1);
  await expect(gateway).toHaveAttribute('aria-disabled', 'true');
});
```

- [ ] **Step 7: `phase-b/zone-disabled.spec.ts` (R3)**

```ts
// SPDX-License-Identifier: Apache-2.0
//
// AC 2 after `helm upgrade` with the gateway zone off (spec § 6.4). The nav must render first (the
// in-phase control), so a count of 0 cannot come from a page that failed. The locator ignores the
// state, so R3 also fails if the entry comes back as an available <a>. R3 cannot tell whether the
// zone-map rule (lib/nav.ts:38-41) or the `absent` rule removed it; both are the chart's contract
// (parent D6). The last clause, "the gateway console Deployment does not exist", runs in
// ci/kind/run.sh specs b.
import { expect, test } from '@playwright/test';
import { TRAEFIK_404_BODY, loginAt } from '../support/login';

test('R3: with the gateway zone disabled, the zone leaves no trace (AC 2)', async ({ page }) => {
  await loginAt(page, '/iam/orgs');
  const nav = page.getByRole('navigation', { name: 'Primary' });
  await expect(nav.getByRole('link', { name: 'Organizations', exact: true })).toBeVisible();
  await expect(nav.getByRole('link', { name: 'Gateway', exact: true })).toHaveCount(0);

  const response = await page.goto('/gateway/overview');
  if (response === null) throw new Error('page.goto(/gateway/overview) returned no response');
  expect(response.status()).toBe(404);
  // Traefik's own body, so a Next not-found page from a surviving gateway console cannot pass.
  expect((await response.text()).trim()).toBe(TRAEFIK_404_BODY);
});
```

- [ ] **Step 8: The Moon input exclusions**

- `ts/apps/iam-console/moon.yml`: after line 263 (`      - 'tests/**/*'` in `test`) and after line 336 (`      - 'tests/**/*'` in `test-e2e`) insert:

```yaml
      # SMA-513 PR 3: the kind-only cluster specs. This task never runs them; they stay in
      # `typecheck` (and in ts:lint's `tests` group), which DO read them.
      - '!tests/cluster/**'
```

- `ts/apps/gateway-console/moon.yml`: after line 358 (`      - '!/ts/apps/iam-console/tests/fixtures/*/next-env.d.ts'`) insert:

```yaml
      # SMA-513 PR 3: the iam-console cluster specs run only in the kind job, never in this tier.
      - '!/ts/apps/iam-console/tests/cluster/**'
```

- [ ] **Step 9: Run, expect PASS**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
moon run iam-console-ts:typecheck; echo "rc=$?"
moon run ts:lint; echo "rc=$?"
moon run ts:fmt; echo "rc=$?"
pnpm --dir ts/apps/iam-console exec vitest list; echo "rc=$?"
pnpm --dir ts/apps/iam-console exec playwright test --config tests/cluster/playwright.config.ts --list; echo "rc=$?"
moon query tasks --affected --json 2>/dev/null >/private/tmp/claude-501/sma-513-pr3/affected.json; echo "rc=$?"
```

Expected: typecheck rc 0; lint rc 0 (if ESLint reports a rule on a new file, fix the file and re-run; do not disable the rule); `ts:fmt` rc 0 (else run Prettier `--write` on the new files and re-run); `vitest list` prints no path under `tests/cluster/` (write the output to a file and `grep -c 'tests/cluster' <file>` → `0`); `playwright … --list` prints five tests: `[phase-a] › phase-a/cross-zone.spec.ts …R2…`, `[phase-a] › phase-a/nav-degraded.spec.ts …R3-control…`, `[phase-a] › phase-a/sso.spec.ts …R1…`, `…R1-control…`, `[phase-b] › phase-b/zone-disabled.spec.ts …R3…`, `Total: 5 tests in 4 files`. For the Moon exclusion: with only a `tests/cluster/**` file changed in the working tree, `moon query tasks --affected` must list `iam-console-ts:typecheck` and must NOT list `iam-console-ts:test`, `iam-console-ts:test-e2e` or `gateway-console-ts:test-e2e` (parse the JSON; take one target per `tasks[project][task]`, root CLAUDE.md). If `moon query tasks` rejects `--json`, drop the flag (bare output is JSON on 2.5.3) and measure its exit status unpiped.

- [ ] **Step 10: Commit**

```bash
git add ts/apps/iam-console/tests/cluster ts/apps/iam-console/moon.yml ts/apps/gateway-console/moon.yml
git commit -m "test(ts): cluster specs R1, R1-control, R2, R3-control and R3 for the kind job (SMA-513)

Two Playwright projects, phase-a and phase-b, under iam-console/tests/cluster.
Chromium only, explicit timeouts, host resolver rules, output outside the app.
The three heavy test tiers exclude tests/cluster from their inputs.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: `.github/workflows/chart.yml`, `EXPECTED_PR_SUBJECTS`, and `ci/CLAUDE.md`

**Files:**
- Create: `.github/workflows/chart.yml`
- Modify: `ci/workflow-credentials/workflow_credentials.py:284-291`
- Modify: `ci/CLAUDE.md:172`

**Interfaces:**
- Consumes: every `run.sh` mode; the SHA pins already in the repo (`images.yml:94,99,102,166`, `ci.yml:156`); kind-action `06c1ae10762d3b9c1644e7fe69596ae519e015a2` (v1.15.0).

**Step times (spec § 7).** Measured on the newest green `images.yml` amd64 leg, run `35895630391` (`gh run view 35895630391 --json jobs`): reclaim 1.62 min, the OCI build of BOTH Rust images 7.95 min, both consoles build + smoke 2.02 min, job 12 min 59 s. `ci.yml` run `35895630361`: `pnpm install` 0.15 min (warm cache), Playwright install 0.53 min. Each step timeout below is about 2.5× the expected time, and the job keeps `timeout-minutes: 60`: `up` 15 (Keycloak's JVM start and realm import dominate), `images` 25 (one cold `--release` IAM build plus two Next builds, measured at up to 10 min together), `install a` 12 (inside helm's own 10 m), `specs a` 10, `upgrade b` 12, `specs b` 8, `diagnose` 5, `down` 5.

- [ ] **Step 1: Failing test first**

Create `.github/workflows/chart.yml` (content in Step 2), then:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
bash ci/workflow-credentials/run.sh; echo "rc=$?"
```

Expected: a row saying the discovered subjects `['chart.yml', 'ci.yml', …]` differ from `EXPECTED_PR_SUBJECTS … re-baseline EXPECTED_PR_SUBJECTS deliberately`, rc 1.

- [ ] **Step 2: The workflow**

```yaml
# SPDX-License-Identifier: Apache-2.0
name: chart

on:
  workflow_dispatch:

  # POST-merge. Broad paths, so a regression anywhere in the code this job tests starts a run.
  push:
    branches:
      - main
    paths:
      - 'rs/**'
      - 'ts/**'
      - 'charts/**'
      - 'ci/kind/**'
      - 'ci/images/**'
      - 'ci/helm-render/**'
      - '.prototools'
      - '.proto/plugins/helm.toml'
      - '.github/workflows/chart.yml'

  # PRE-merge: only this job's own inputs, so a console PR does not pay for a cold --release IAM
  # build (the SMA-520 reason in images.yml). Any other PR can start the job by hand:
  # `gh workflow run chart.yml --ref <branch>`.
  pull_request:
    branches:
      - main
    paths:
      - 'charts/**'
      - 'ci/kind/**'
      - 'ts/apps/iam-console/tests/cluster/**'
      - '.github/workflows/chart.yml'

# Nothing is pushed and nothing is published: kind loads the images locally (parent spec § 11).
permissions:
  contents: read

concurrency:
  # `event_name` is in the GROUP so a manual dispatch cannot cancel a running push job.
  group: chart-${{ github.workflow }}-${{ github.ref }}-${{ github.event_name }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}

jobs:
  kind:
    name: kind install + upgrade (amd64)
    # NOT a required check (spec § 7): a broken chart reds main, the same trade images.yml makes.
    runs-on: ubuntu-latest
    timeout-minutes: 60
    env:
      # SMA-609: every captured shim output must set this.
      PROTO_REPORTER: text
    steps:
      # Same reclaim as images.yml: this job builds IAM in --release on a ~14 GB disk.
      - name: Reclaim runner disk (drop unused preinstalled toolchains)
        run: |
          df -h /
          sudo rm -rf /usr/local/lib/android /usr/share/dotnet /opt/ghc /usr/local/.ghcup \
            /opt/hostedtoolcache/CodeQL || true
          sudo docker image prune --all --force > /dev/null 2>&1 || true
          df -h /

      - name: Checkout
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1
        with:
          persist-credentials: false

      # ci/images/run.sh builds with `docker buildx build --load`, measured under this builder in
      # images.yml.
      - name: Set up Buildx
        uses: docker/setup-buildx-action@f87e5991a6d7451dcb8d9637bfbc97413f497069  # v4.4.1

      - name: Set up proto (pinned via .prototools)
        uses: moonrepo/setup-toolchain@261c62cb5b0f580c7be7c8cd0f023a2e96756095  # v0
        with:
          cache: false

      # Narrow installs: this job needs only these three.
      - name: Install helm, node and pnpm
        run: |
          proto install helm
          proto install node
          proto install pnpm

      - name: Resolve the pnpm store path
        id: cache-paths
        run: echo "pnpm-store=$(pnpm store path)" >> "$GITHUB_OUTPUT"

      - name: Cache pnpm store
        uses: actions/cache@55cc8345863c7cc4c66a329aec7e433d2d1c52a9  # v6.1.0
        with:
          path: ${{ steps.cache-paths.outputs.pnpm-store }}
          key: pnpm-${{ runner.os }}-${{ hashFiles('ts/pnpm-lock.yaml') }}
          restore-keys: |
            pnpm-${{ runner.os }}-

      - name: Install JS workspace dependencies
        run: pnpm --dir ts install --frozen-lockfile

      # From the PACKAGE: `playwright` is only on that package's node_modules/.bin (ci.yml).
      - name: Install Playwright Chromium
        run: pnpm --dir ts/apps/iam-console exec playwright install --with-deps chromium

      # kind v0.31.0 is the newest kind with a Kubernetes 1.31 node image (the golden files render
      # for 1.31.0); kubectl matches the node's minor. No cluster here: run.sh up makes it.
      - name: Install kind and kubectl
        uses: helm/kind-action@06c1ae10762d3b9c1644e7fe69596ae519e015a2  # v1.15.0
        with:
          install_only: true
          version: v0.31.0
          kubectl_version: v1.31.14

      - name: Cluster up (kind, Traefik, CA, CoreDNS, Postgres, Redis, Keycloak, preflight)
        timeout-minutes: 15
        run: bash ci/kind/run.sh up

      - name: Build and load the three images
        timeout-minutes: 25
        run: bash ci/kind/run.sh images

      - name: helm install (phase A, both zones)
        timeout-minutes: 12
        run: bash ci/kind/run.sh install a

      - name: Specs, phase A (R1, R1-control, R2, R3-control)
        timeout-minutes: 10
        run: bash ci/kind/run.sh specs a

      - name: helm upgrade (phase B, gateway zone off) and settle
        timeout-minutes: 12
        run: bash ci/kind/run.sh upgrade b

      - name: Specs, phase B (R3)
        timeout-minutes: 8
        run: bash ci/kind/run.sh specs b

      # `cancelled()` too: a step or job timeout cancels, and a hang is when evidence matters most.
      - name: Collect evidence
        if: failure() || cancelled()
        timeout-minutes: 5
        run: bash ci/kind/run.sh diagnose

      - name: Upload evidence
        if: failure() || cancelled()
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a  # v7.0.1
        with:
          name: kind-evidence
          path: ${{ runner.temp }}/paigasus-kind/diagnose
          if-no-files-found: warn
          # Short: the Playwright traces hold the per-run (throwaway) password.
          retention-days: 7

      - name: Cluster down
        if: always()
        timeout-minutes: 5
        run: bash ci/kind/run.sh down
```

- [ ] **Step 3: Register the subject and fix the count**

`ci/workflow-credentials/workflow_credentials.py:284-291`: replace

```python
EXPECTED_PR_SUBJECTS = (
    "ci.yml",
```

with

```python
EXPECTED_PR_SUBJECTS = (
    "chart.yml",
    "ci.yml",
```

`ci/CLAUDE.md:172`: replace `pin of the five subject` with `pin of the seven subject`.

- [ ] **Step 4: Run, expect PASS — workflow credentials and actionlint (bash 5, pipe preflight)**

```bash
bash ci/workflow-credentials/run.sh --negative-control; echo "rc=$?"
bash ci/workflow-credentials/run.sh; echo "rc=$?"
/opt/homebrew/bin/bash ci/actionlint/run.sh; echo "rc=$?"
```

Expected: both workflow-credentials runs rc 0. For actionlint, FIRST read the preflight line on stderr: `actionlint gate: pipe capacity 65536 bytes (floor 8192)`. Then every check prints ok, check 5 accepts all nine `push` and four `pull_request` globs of `chart.yml`, check 13 finds nothing in `ci/kind/run.sh` or `chart.yml`, rc 0. If the preflight instead exits rc 2 with a `small` pipe message, this host gives no local actionlint verdict (root CLAUDE.md): record that and rely on CI's `repo:actionlint`.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/chart.yml ci/workflow-credentials/workflow_credentials.py ci/CLAUDE.md
git commit -m "ci(repo): chart.yml, the kind install and upgrade job (SMA-513)

Not a required check. Split triggers as in images.yml, contents: read only,
every action pinned by SHA, a timeout on every step, and the evidence upload
on failure or cancel. chart.yml is registered first in EXPECTED_PR_SUBJECTS.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Dependabot for the kind manifests

**Finding (spec § 8 item 9).** Read from dependabot-core `main` at `9743eeaa4fcb6d7b13ff5f87c850d23363c77ba6`:

- `docker/lib/dependabot/docker/file_fetcher.rb:25-28`: `required_files_in?` accepts a Dockerfile **or** any file that matches `YAML_REGEXP`; the error text is "Repo must contain a Dockerfile, Containerfile, or Kubernetes YAML files."
- `docker/lib/dependabot/shared/shared_file_fetcher.rb` (`YAML_REGEXP = /^[^\.].*\.ya?ml$/i`; `yamlfiles` reads `repo_contents` of the configured directory, not its subdirectories; `correctly_encoded_yamlfiles` keeps a file when `YAML.safe_load` of it — the FIRST document only, as its own comment says — is a Hash with `apiVersion` and `kind`, or when it is a likely Helm chart).
- `docker/lib/dependabot/docker/file_parser.rb:88-137`: `workfile_file_dependencies` splits the file on `^---$`, loads every document and walks it for `image:` keys at any depth; `IMAGE_SPEC` accepts `name:tag@sha256:<64 hex>`.

**So the `docker` ecosystem DOES read the manifests.** Add a `/ci/kind/manifests` entry. Residual 2 shrinks to: the Traefik chart and image, the kind node image, and the copies in `ci/images/run.sh:50-51` are not covered; each carries its refresh command.

**Files:**
- Modify: `.github/dependabot.yml` (append after the `/ts` docker entry, end of file)

**Interfaces:** none.

- [ ] **Step 1: Failing check first**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
PY="$(uv run --locked --project ci/helm-render python3 -c 'import sys; print(sys.executable)')"
"$PY" -c 'import yaml; u = yaml.safe_load(open(".github/dependabot.yml"))["updates"]; print([e["directory"] for e in u if e["package-ecosystem"] == "docker"])'
```

Expected: `['/rs', '/ts']` (no `/ci/kind/manifests`).

- [ ] **Step 2: Append the entry**

```yaml

  # SMA-513 PR 3: the kind job's dependencies (Postgres, Redis, Keycloak, the preflight curl), pinned
  # by digest in plain Kubernetes manifests. Read from dependabot-core main at 9743eeaa: the docker
  # ecosystem fetches every *.yml/*.yaml file directly in `directory` whose FIRST document has
  # `apiVersion` and `kind` (docker/lib/dependabot/shared/shared_file_fetcher.rb), and parses every
  # `image: <name>:<tag>@sha256:<digest>` in it (docker/lib/dependabot/docker/file_parser.rb). So each
  # manifest there must start with a Kubernetes object, never with `---`. NOT covered: the Traefik
  # chart and image and the kind node image (ci/kind/run.sh, ci/kind/traefik-values.yaml), and the
  # copies of the Postgres and curl pins in ci/images/run.sh:50-51; each carries its refresh
  # command. Version bumps stay human: a Keycloak minor can change the realm import and the hostname
  # and proxy options, and Postgres 16 matches the IAM smoke test.
  - package-ecosystem: docker
    directory: /ci/kind/manifests
    schedule:
      interval: weekly
      day: monday
      time: "06:00"
      timezone: Etc/UTC
    commit-message:
      prefix: "build(deps)"
      prefix-development: "build(deps)"
    groups:
      docker-minor-patch:
        applies-to: version-updates
        update-types:
          - minor
          - patch
    ignore:
      - dependency-name: "quay.io/keycloak/keycloak"
        update-types:
          - "version-update:semver-major"
          - "version-update:semver-minor"
      - dependency-name: "postgres"
        update-types:
          - "version-update:semver-major"
      - dependency-name: "redis"
        update-types:
          - "version-update:semver-major"
```

- [ ] **Step 3: Run, expect PASS**

```bash
"$PY" -c 'import yaml; u = yaml.safe_load(open(".github/dependabot.yml"))["updates"]; print([e["directory"] for e in u if e["package-ecosystem"] == "docker"])'
/opt/homebrew/bin/bash ci/actionlint/run.sh; echo "rc=$?"
```

Expected: `['/rs', '/ts', '/ci/kind/manifests']`; actionlint rc 0 (or the recorded small-pipe rc 2, as in Task 10). After merge, the Dependabot "Last checked" status for `/ci/kind/manifests` in the repository's Insights → Dependency graph → Dependabot tab must show no "No Dockerfiles nor Kubernetes YAML found" error; record it in the PR as a post-merge check.

- [ ] **Step 4: Commit**

```bash
git add .github/dependabot.yml
git commit -m "ci(repo): Dependabot for the kind job's manifest digests (SMA-513)

The docker ecosystem reads Kubernetes YAML whose first document is an object
with apiVersion and kind, and parses digest-pinned image lines.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12: The chart runbook and the CLAUDE.md notes

All text in ASD-STE100 Simplified Technical English: short sentences, active voice, no idioms.

**Files:**
- Create: `docs/ops/RUNBOOK-chart.md`
- Create: `charts/CLAUDE.md`
- Modify: `ts/CLAUDE.md` (end of `## Build and module resolution`, before `## Auth, sessions and the console packages` at line 134)
- Modify: `CLAUDE.md:53` (the memory-map table; insert a row after `contracts/CLAUDE.md`)

**Interfaces:** none.

- [ ] **Step 1: `docs/ops/RUNBOOK-chart.md`**

```markdown
# Chart RUNBOOK — `charts/paigasus` (SMA-513)

Operator reference for the Paigasus Helm chart. It installs the IAM console, the AI Gateway
console and the IAM backend behind one ingress origin. The design is in
`docs/superpowers/specs/2026-09-19-sma-513-multi-zone-ingress-helm-design.md` and its two
addenda. `charts/paigasus/README.md` holds the developer detail.

## 1. Values reference

| Key | Required | Meaning |
| -- | -- | -- |
| `zones.<id>.enabled` | — | Turns a zone on or off. It controls six projections together (§ 4) |
| `zones.<id>.basePath` | — | The zone's path prefix. It must start with `/`, must not end with `/`, and must be unique |
| `zones.<id>.console.image.{repository,tag}` | — | The console image. `tag` defaults to the chart `appVersion` |
| `zones.<id>.console.replicas` | — | Console replicas (default 2) |
| `zones.iam.backend.image.{repository,tag}` | — | The IAM image |
| `zones.iam.backend.apiKeysPepperSecret` | yes | A Secret with key `pepper`: base64 of at least 32 bytes |
| `zones.iam.backend.apiKeysSecretVersion` | no | Change it after you rotate the pepper Secret, so the IAM pod restarts |
| `zones.gateway.backend.url` | when `gateway` is on | The base URL of an existing gateway backend. The chart does not deploy it |
| `ingress.host` | yes | The one public host. `PAIGASUS_PUBLIC_ORIGIN` is `https://<host>` |
| `ingress.className` | no | The IngressClass of your controller |
| `ingress.tlsSecretName` | yes | The TLS Secret for `ingress.host`. The ingress must end TLS |
| `ingress.annotations` | no | Extra annotations. Do not add a rewrite annotation (§ 3) |
| `oidc.issuer` | yes | The IdP issuer URL. It must be `https` |
| `oidc.clientId` | yes | The console's OIDC client. IAM also uses it as the access-token audience (§ 6) |
| `oidc.existingSecret` | yes | A Secret with keys `oidc-client-secret` and `session-redis-url` |
| `oidc.secretVersion` | no | Change it after the Secret changes, so the console pods restart |
| `oidc.caBundle.existingConfigMap` | no | A ConfigMap with the PEM root certificates of a private IdP CA (§ 7) |
| `oidc.caBundle.key` | no | The key in that ConfigMap (default `ca.crt`) |
| `oidc.caBundle.version` | no | Change it after the ConfigMap changes, so all three Deployments restart |
| `postgres.existingSecret` | yes | A Secret with key `database-url`: the complete Postgres DSN |
| `postgres.secretVersion` | no | Change it after the Secret changes, so the IAM pod restarts |

## 2. Refused combinations

`paigasus.validate` in `templates/_helpers.tpl` stops the render with its own message. The chart
refuses:

- no enabled zone;
- the `gateway` zone without the `iam` zone;
- a zone id that is not a known service slug;
- a `basePath` that is empty, has no leading `/`, ends with `/`, or is used by two zones;
- an empty value for each required key in § 1;
- `zones.iam.backend.deploy: false` (an external IAM is not supported);
- `zones.gateway.backend.deploy: true` (the chart cannot run the gateway backend);
- `zones.<id>.backend.url` empty when the chart does not deploy that backend;
- `oidc.caBundle.existingConfigMap` set with an empty `oidc.caBundle.key`.

## 3. No rewrite annotation

Do not add a rewrite annotation to the ingress. Each console has its `basePath` compiled in and
serves its full path. A rewrite that removes `/iam` breaks every route in that zone.

## 4. One values block, six projections, and the zone-map upgrade

A zone's `enabled` flag controls six things together: its ingress rule, its `PAIGASUS_ZONES`
entry, its `PAIGASUS_SERVICES` entry, `PAIGASUS_IAM_GRPC_URL`, its console Deployment and Service,
and its backend Deployment and Service. So a zone cannot be routed but not shown, or shown but not
routed. This is decision D6.

**The limit of D6.** The chart cannot express "the gateway zone is shown but not routed", or the
reverse. That is on purpose.

**Upgrade order for a zone-map change.** One `helm upgrade` changes the Ingress, the zone-map
ConfigMap and the Deployments at the same time. Helm does not order them. The consoles restart
because the `checksum/zonemap` annotation changes. During the restart:

- When you remove a zone, its ingress rule goes away at once. Old console pods of the other zones
  still show a link to it until their replacements are Ready. That link then gives the
  controller's 404.
- When you add a zone, the new zone can be Ready before the old pods of the other zones show it.

Do not test the result until no old console pod exists. `helm upgrade --wait` waits for the new
pods, not for the old pods to go. The kind job waits for both (`ci/kind/run.sh`, the settle step).

## 5. What restarts what

| You change | Restarts | Why |
| -- | -- | -- |
| a value that changes a console's zone map or env | the consoles | `checksum/zonemap`, `checksum/console-env` |
| the contents of `oidc.existingSecret` | nothing, until you change `oidc.secretVersion` | the chart cannot see the Secret |
| the contents of `postgres.existingSecret` or the pepper Secret | nothing, until you change its version value | the same |
| the contents of the CA ConfigMap | nothing, until you change `oidc.caBundle.version` | Node and IAM read the file once, at start |

A change of `oidc.caBundle.version` restarts every pod that mounts the bundle: both consoles and
IAM. That is expected, as for `oidc.secretVersion`.

## 6. The IdP contract

IAM validates the **access token** that the console sends with each gRPC call. It does not
validate the ID token. The login callback calls `authn.whoAmI` with the access token. A refused
token does NOT stop the login. The console then shows IAM as unusable. So a wrong IdP setup shows
late and unclearly. Check these four items before you install:

1. **Audience.** The access token's `aud` claim must contain `oidc.clientId`. The chart sets IAM's
   accepted audience to `oidc.clientId`, and no other value (an `oidc.audience` value is future
   work).
2. **Email.** The access token must carry an `email` claim. IAM creates the principal on the first
   login from it.
3. **Algorithm.** The token must be signed with RS256 or ES256, and its header must carry a `kid`.
4. **Discovery.** The discovery document's `issuer` must equal `oidc.issuer`, and its `jwks_uri`
   must be `https`.

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

## 7. An IdP with a private CA (`oidc.caBundle`)

The consoles and IAM refuse an issuer that is not `https`. If the IdP certificate chain ends at a
private CA, put the CA's PEM root certificates into a ConfigMap and set
`oidc.caBundle.existingConfigMap`. A CA certificate is public, so it is a ConfigMap, not a Secret.

```bash
kubectl -n <namespace> create configmap paigasus-idp-ca --from-file=ca.crt=<your-root-ca.pem>
helm upgrade <release> charts/paigasus --reuse-values \
  --set oidc.caBundle.existingConfigMap=paigasus-idp-ca
```

The chart then mounts the key read-only under `/etc/paigasus/idp-ca/` in every console pod and in
the IAM pod. The consoles get `NODE_EXTRA_CA_CERTS`. IAM gets `IAM_AUTHN__EXTRA_CA_BUNDLE_PATH`.

**Failure modes.**

- **A missing ConfigMap or a missing key.** Every pod that mounts it stops at volume setup, with a
  `FailedMount` event. The pod does not start. Read `kubectl describe pod`.
- **A file with no PEM certificate.** IAM does not start. Its log names the bundle path.
- **A key whose content is not a valid PEM certificate, for the consoles.** This case fails OPEN.
  Node prints a warning and starts. The failure shows only at login, as a TLS error against the
  IdP in the console log. After a change, read the console log for a `NODE_EXTRA_CA_CERTS` warning.
- **Scope.** The consoles trust the bundle for EVERY TLS connection, Redis included. IAM trusts it
  only for JWKS fetches. Put only the roots the IdP needs in the bundle.
- **Rotation.** After you change the ConfigMap, change `oidc.caBundle.version`. Node and IAM read
  the bundle only at start.

## 8. The kind job

`.github/workflows/chart.yml` installs this chart into kind and runs the Playwright specs in
`ts/apps/iam-console/tests/cluster/`. It is not a required check. It uses **Traefik** as the
ingress controller, because the Kubernetes project retired ingress-nginx (announced on 2025-11-11;
the repository was archived on 2026-03-24). The chart renders a plain `networking.k8s.io/v1`
Ingress with no controller annotation, so it works with any controller that serves that API.
Set `ingress.className` to your controller's class.

To run the job by hand on a branch: `gh workflow run chart.yml --ref <branch>`. To run it locally,
see `ci/kind/README.md`. The job maps the two host names with a CoreDNS `hosts` block. That is a
kind-only device: a real cluster needs real DNS for the IdP host.
```

- [ ] **Step 2: `charts/CLAUDE.md`**

```markdown
# paigasus-core — `charts/`

Project memory for the Helm chart. Claude Code loads this file only when it reads a file here.
Operator detail is in `docs/ops/RUNBOOK-chart.md`; developer detail in `charts/paigasus/README.md`.

- **The ingress has no rewrite annotation, on purpose.** Each console has its `basePath` compiled
  in. A rewrite that removes `/iam` breaks every route in that zone. Do not add one to
  `templates/ingress.yaml` or to `values.yaml`.
- **One values block renders three projections that must agree:** `PAIGASUS_ZONES`,
  `PAIGASUS_SERVICES` and the ingress rules. All three come from `zones.<id>.enabled` through one
  `range`. Do not write a zone literally in a template: `repo:helm-render` check 1 and check 2
  fail on it.
- **Check 1a couples the chart to `contracts/proto/paigasus/common/v1/service_info.proto`.**
  `paigasus.serviceSlugs` in `_helpers.tpl` must EQUAL the slugs of `enum Capability`. A PR that adds
  a capability for a new service must also edit `_helpers.tpl`.
- **The chart-script floor is pinned twice.** `ci/helm-render/run.sh` `CHART_SCRIPT_FLOOR=7` and
  `ci/affected-graph/ci_targets.py` `HELM_RENDER_SH_CALL_SITES` (substring match). Change both in
  one commit, or `repo:affected-smoke` goes red.
- **A new chart script under `tests/` runs in CI through the `tests/*.sh` glob.** Raise the floor
  with it, and fix the counts in `ci/helm-render/README.md` and `helm_render.py`.
- **Four negative-control fixtures are whole-file copies** of `templates/_helpers.tpl` (two) and
  `templates/console-deployment.yaml` (two), under `ci/helm-render/fixtures/`. An edit to either
  live file must re-sync them in the SAME commit, keeping each mutation. A stale `_helpers.tpl`
  fixture has none of the new helpers, so its renders fail and the control reports INCONCLUSIVE.
- **A YAML comment in a template renders into the manifest,** so it is part of the golden files.
  Put a comment for a conditional block INSIDE the `{{- if }}` block, or the default render and the
  goldens change.
- **A new REQUIRED value must also go into** the seven chart scripts' value lists,
  `helm_render.py` `STUB_VALUES`, and `ci/kind/values/a.yaml` (row 7).
- **Read optional nested values with `dig`**, as the `paigasus.idpCa*` helpers do. A release made
  before the value existed has no map under `--reuse-values`.
```

- [ ] **Step 3: `ts/CLAUDE.md` link line**

Before line 134 (`## Auth, sessions and the console packages`), insert:

```markdown
- The console image rules live in two places already; do not copy them here: the static-asset
  staging rule is `docs/ops/RUNBOOK-containers.md:364-369`, and the exec-form `ENTRYPOINT`/`HEALTHCHECK`
  rule (no `ARG`/`ENV` expansion) is `rs/CLAUDE.md:207` and `docs/ops/RUNBOOK-containers.md:317-322`.
  The kind-only specs in `apps/iam-console/tests/cluster/` run only from `ci/kind/run.sh specs a|b`.

```

- [ ] **Step 4: Root `CLAUDE.md` memory-map row**

After line 53 (`| \`contracts/CLAUDE.md\` | codegen drift and the FFI bindings |`), insert:

```markdown
| `charts/CLAUDE.md` | the Helm chart: no rewrite, the three projections, the helm-render pins |
```

This row is outside both gated marker blocks (`ci-targets`, `moon-diagnosis`). Check that neither marker moved:

```bash
grep -c 'ci-targets:begin' CLAUDE.md; grep -c 'moon-diagnosis:begin' CLAUDE.md
```

Expected: `1` and `1`.

- [ ] **Step 5: Verify and commit**

```bash
/bin/bash ci/affected-graph/run.sh; echo "rc=$?"
git add docs/ops/RUNBOOK-chart.md charts/CLAUDE.md ts/CLAUDE.md CLAUDE.md
git commit -m "docs(repo): chart runbook, charts/CLAUDE.md and the memory-map row (SMA-513)

The runbook states the IdP access-token contract with the Keycloak audience
mapper, the CA bundle failure modes, and the zone-map upgrade order.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

Expected: affected-graph rc 0 (it reads root `CLAUDE.md`).

---

### Task 13: PR-level verification (spec § 10)

**Files:** none new. Temporary commits in Steps 5 and 6 are reverted with `git revert` (new commits, never amend, never force-push).

- [ ] **Step 1: The full gate graph, locally, with the bash split**

Run the root `CLAUDE.md` command once, under the default `bash` on `PATH` (Homebrew 5.3.15):

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
git fetch origin main
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :helm-render :test-e2e \
  --base origin/main \
  --include-relations; echo "rc=$?"
```

Then re-run the gates that need the other bash directly and read THOSE results instead of the `moon ci` verdict for them:

```bash
/bin/bash ci/affected-graph/run.sh --negative-control; echo "affected-smoke control rc=$?"
/bin/bash ci/affected-graph/run.sh; echo "affected-smoke rc=$?"
/opt/homebrew/bin/bash ci/ruff/run.sh; echo "ruff rc=$?"
/opt/homebrew/bin/bash ci/next-public/run.sh; echo "next-public rc=$?"
/opt/homebrew/bin/bash ci/publish-metadata/run.sh; echo "publish-metadata rc=$?"
/opt/homebrew/bin/bash ci/actionlint/run.sh; echo "actionlint rc=$?"
/bin/bash ci/helm-render/run.sh --self-test; /bin/bash ci/helm-render/run.sh --negative-control; /bin/bash ci/helm-render/run.sh; echo "helm-render 3.2 rc=$?"
/opt/homebrew/bin/bash ci/helm-render/run.sh; echo "helm-render 5 rc=$?"
bash ci/workflow-credentials/run.sh; echo "workflow-credentials rc=$?"
bash charts/paigasus/tests/render.sh; git diff --stat origin/main -- charts/paigasus/tests/golden
```

Expected: every direct run rc 0. For actionlint, read the preflight line first (`pipe capacity 65536 bytes`); if it reports a small pipe (rc 2), record "no local verdict" and use CI. The golden diff against `origin/main` shows only Task 3's two lines. If `moon ci` reds a task that one of the direct runs passes, that is the known bash split, not a finding (root CLAUDE.md; memory "gate failures from the wrong bash"). If `moon ci` stops on an unattributed failure, follow the `moon-diagnosis` procedure in root `CLAUDE.md` (Step 0 first).

- [ ] **Step 2: Push and read the first `chart.yml` run**

```bash
git push -u origin feature/sma-513-kind-chart-job
gh run list --workflow chart.yml --branch feature/sma-513-kind-chart-job --limit 1 --json databaseId,status,conclusion,url
```

Watch it with Monitor (it notifies on expiry), not a background sleep loop. Expect more than one CI round (spec § 10). On a red: download `kind-evidence` (`gh run download <id> -n kind-evidence -D /private/tmp/claude-501/sma-513-pr3/evidence-<id>`), read `README.md`'s "Where to look first", fix with a NEW commit, push. If `images` fails in `load_images` with the `docker-image` path, apply the Task 7 Step 1 decision rule (`archive`). Record each round's cause and fix in the PR body.

- [ ] **Step 3: Record the green run and its step times**

```bash
gh run view <green-run-id> --json jobs --jq '.jobs[] | .steps[] | [.name, .startedAt, .completedAt] | @tsv'
```

Put the step durations next to the step timeouts in the PR body. If any step used more than 60% of its timeout, raise that timeout in a new commit and say why.

- [ ] **Step 4: The images.yml baseline, for the PR body**

```bash
gh run list --workflow images.yml --status success --limit 1 --json databaseId
gh run view <id> --json jobs --jq '.jobs[] | select(.name | test("amd64")) | .steps[] | [.name, .startedAt, .completedAt] | @tsv'
```

Expected today: run `35895630391`, "Build both images as OCI archives" 7.95 min, "Build + smoke both consoles" 2.02 min. Put it beside Step 3's `images` time.

- [ ] **Step 5: R3 delete-the-feature proof (CI, temporary commit)**

With Edit in `.github/workflows/chart.yml`, add `if: false` to the step `helm upgrade (phase B, gateway zone off) and settle` (a new line under `timeout-minutes: 12`). Commit and push:

```bash
git add .github/workflows/chart.yml
git commit -m "ci(repo): TEMPORARY, skip the phase-B upgrade to prove R3 goes red (SMA-513)

Reverted by the next commit.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push
```

Expected run: "Specs, phase B" fails. The Playwright report names R3 failing at `toHaveCount(0)` with `Received: 1` (the in-phase control `Organizations` passes), and `run.sh` also prints `FAIL [R3]: the gateway console Deployment still exists`. Record the run URL and the assertion line. Then:

```bash
git revert --no-edit HEAD
git push
```

- [ ] **Step 6: R1 delete-the-feature proof (CI, temporary commit)**

With Edit in `ts/apps/iam-console/tests/cluster/phase-a/sso.spec.ts`: change the R1 test signature `async ({ page, context }) => {` to `async ({ page, browser }) => {`, and `const second = await context.newPage();` to `const second = await (await browser.newContext({ baseURL: 'https://console.paigasus.test', ignoreHTTPSErrors: true })).newPage();`. Run `moon run iam-console-ts:typecheck ts:lint` (expect rc 0), commit (`test(ts): TEMPORARY, point R1's second page at a new context to prove R1 goes red (SMA-513)` with the trailer), push.

Expected run: "Specs, phase A" fails at R1. The first assertion in the test that can fail is `expect(response.request().redirectedFrom(), 'the document must arrive in one hop, with no redirect').toBeNull()`: a new context has no cookie, so the gateway zone redirects. Record the run URL and the failing line. Then `git revert --no-edit HEAD` and push.

The new context still resolves both hosts: `--host-resolver-rules` is a browser launch argument, and every context of that browser uses it.

- [ ] **Step 7: The final green run**

After both reverts, the `chart.yml` run on the branch head is green, and CI's required checks are green. Record in the PR body:

- the green `chart.yml` run URL and its step times (Step 3);
- the two delete-the-feature runs (Steps 5 and 6) with their failing lines;
- `ca-bundle.sh` green under `/bin/bash` 3.2.57 and Homebrew bash 5.3.15, and its delete-the-feature result (Task 2 Step 9);
- the checks 3 and 4 run with the value set (Task 4 Step 6);
- the row 7 proofs (Task 5 Step 7) and the negative control after the four re-syncs;
- the image-load measurement (Task 7 Step 1) and the local bring-up result (Task 7 Step 5);
- the golden diff: only Task 3's comment line;
- the follow-up to file: an `oidc.audience` value (spec § 11).

---

## Spec defects found while planning

1. **The golden files DO change (spec § 5.3 and § 10 vs § 9).** The comment that § 9 corrects is a YAML comment inside `backend-deployment.yaml`, so it renders into the manifest: `grep -n "ID tokens" charts/paigasus/tests/golden/*.yaml` → `iam-only.yaml:149`, `iam-and-gateway.yaml:164`. The spec says both "correct the comment" and "the golden files are unchanged". **Planned around:** Task 3 makes the correction in its own commit, re-baselines with `render.sh --update`, and requires the golden diff to be exactly that one line in each file. Every other task keeps the goldens byte-identical.

2. **Four fixtures, not two, are whole-file copies of files this PR edits (spec § 5.5).** `ci/helm-render/fixtures/slug-mirror/templates/_helpers.tpl` and `ci/helm-render/fixtures/zones-omits-enabled/templates/_helpers.tpl` also copy a changed file. Because the new templates call `include "paigasus.idpCaConfigMap"`, a stale `_helpers.tpl` fixture fails every render and the negative control reports INCONCLUSIVE (rc 2). **Planned around:** Task 1 re-syncs both `_helpers.tpl` fixtures in the same commit as the `_helpers.tpl` change; Task 2 re-syncs the two named ones.

3. **kind-action's default kind has no Kubernetes 1.31 node image (spec § 4.1).** `helm/kind-action` v1.15.0 installs kind v0.33.0 and kubectl v1.37.0 by default; kind v0.32.0 and v0.33.0 ship no 1.31 image. The newest kind with a 1.31 node is v0.31.0 (`kindest/node:v1.31.14@sha256:6f86…ac2b`). kubectl v1.37 against a 1.31 server is outside the ±1-minor skew policy. **Planned around:** `chart.yml` passes `version: v0.31.0` and `kubectl_version: v1.31.14` to kind-action; `run.sh` pins the node image by digest.

4. **R1-control's `request.get(…, { maxRedirects: 0 })` cannot resolve the host (spec § 6.3).** Playwright's `APIRequestContext` (the `request` fixture and `page.request`) sends from Node, which does not see Chromium's `--host-resolver-rules`, and the job does not edit `/etc/hosts` (B4). The request would fail with `ENOTFOUND console.paigasus.test`. **Planned around:** R1-control navigates in a fresh browser context with no cookie and asserts the first two hops of the redirect chain (`/gateway/overview` 3xx → `/gateway/auth/login`), the same property. The Playwright config comment forbids the Node-side request API against the two hosts.

5. **The new `repo:helm-render` Moon input is pinned a second time (spec § 5.5 omits it).** `ci/affected-graph/ci_targets.py:390-398` (`SELF_TASK_EXPECTED_GLOBS["helm-render"]`) must equal the task's inputs, or `repo:affected-smoke` reds. **Planned around:** Task 5 edits both. The spec's glob `ci/kind/values/**` is written `ci/kind/values/**/*`, the form every other input in `moon.yml` uses.

6. **More stale counts than § 5.5 lists.** `moon.yml:980` ("holds six scripts"), `charts/paigasus/README.md:130` ("runs all six"), and `helm_render.py:937-938` (the self-test pins `EXPECTED_ROW_LABELS` at 20 labels, which row 7 makes 21). **Planned around:** Tasks 2, 4 and 5 fix them.

7. **R3's "the gateway console Deployment does not exist" (spec § 6.4) has no natural home in a Playwright spec.** It would need `child_process` and `kubectl` in the TS tier. **Planned around:** `run.sh specs b` asserts it after Playwright and reports it as `[R3]`, with rc 1 on failure. The R3 delete-the-feature run (Task 13 Step 5) reds both halves.

8. **A spec-level ambiguity, resolved conservatively:** § 5.2 says the `version` annotation is rendered "only when it is set", without saying whether it also needs `existingConfigMap`. The plan renders it when `version` is set, independent of `existingConfigMap` (harmless, and the simplest reading). `ca-bundle.sh` pins that no annotation renders while `version` is unset.
