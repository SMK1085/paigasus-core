# SMA-513 PR 2b — the `repo:helm-render` gate (spec addendum)

- **Issue:** SMA-513, PR 2b of four.
- **Parent spec:** `2026-09-19-sma-513-multi-zone-ingress-helm-design.md`. This addendum
  replaces parent § 8 where the two disagree. Everything else in the parent stays in force.
- **Revision 2**, after the adversarial challenge. § 12 records what the challenge changed.

---

## 1. Why an addendum

Parent § 8 was written before PR 2a built the chart. PR 2a (PR 273, `4dc3387c`) now exists,
and three facts change § 8:

1. **PR 2a already covers part of § 8.1.** `charts/paigasus/tests/` holds six scripts.
   `render.sh` does `helm lint` and the golden diff with `--kube-version 1.31.0` (check 5).
   `ingress.sh` compares the three key sets and greps for a disabled zone id (checks 1 and 2,
   partly). `maps.sh`, `names.sh`, `env.sh` and `refusals.sh` test other properties. **Nothing
   runs any of the six in CI or in Moon.** They run only by hand.
2. **Gate registration is seven obligations, not eight.** `ci/CLAUDE.md` ("Registering a new
   `repo:*` gate", the `repo:ruff-ci` entry) holds the measured list of seven. Parent obligations
   1–7 match it in order. Parent obligation 8, `EXPECTED_PR_SUBJECTS`, is not an obligation of
   this gate. Only a new `pull_request` workflow triggers it, and that workflow is PR 3's
   `chart.yml`. Obligation 8 moves to PR 3. This PR adds a uv project, which brings three other
   obligations (§ 7).
3. **The chart's `SERVICE_SLUGS` list is an unpinned mirror.** `_helpers.tpl:8-10` hard-codes
   `"iam gateway"` with a comment "keep this list equal to `state.ts`". The TS set is derived at
   run time from the generated proto registry. Nothing compares the two.

## 2. Decisions

| # | Topic | Decision |
| -- | -- | -- |
| A1 | Relation to the chart scripts | The gate runs all six chart scripts unchanged, and adds only what they do not do |
| A2 | Slug mirror | The gate pins `paigasus.serviceSlugs` as EQUAL to the slug set derived from the proto source |
| A3 | Fixture form | Each fixture holds only the files it changes, copied over a temp copy of the live chart |
| A4 | Where checks 1–4 live | One Python module that takes `--chart <dir>` |
| A5 | Obligation 8 | Moves to PR 3 |

**A4, and why checks 1 and 2 exist twice.** The chart scripts could render a fixture chart: a
`cp -R` of the chart also copies `tests/`, and each copied script computes `CHART` from its own
location. So the overlap is a choice, not a constraint. The reasons for the choice:

- The module's checks are stronger than `ingress.sh`'s. Check 1 adds an independent expected set
  and the route target. Check 2 searches every key and value, and uses a sentinel URL (§ 5).
- One module gives each check one named row, so the negative control can require the row it
  names.

`ingress.sh` stays as it is and still runs (check 6).

**A2, why equality and not subset.** The safety property is "chart ⊆ proto". Equality is stronger
on purpose. It makes the contracts PR that registers the first capability of a new service also
edit `charts/`, which is the only moment anyone is prompted to think about a new zone (parent
§ 12 risk 1). The README states this.

## 3. Files

| File | Content |
| -- | -- |
| `ci/helm-render/run.sh` | The three-mode entry point |
| `ci/helm-render/helm_render.py` | Checks 1, 1a, 2, 3, 4 and the in-process self-test rows |
| `ci/helm-render/pyproject.toml` | Pins `pyyaml==6.0.3`, the version in `py/uv.lock`. Same layout as `ci/workflow-credentials/` |
| `ci/helm-render/uv.lock` | Locks it |
| `ci/helm-render/fixtures/<name>/…` | Six overlay fixtures (§ 6) |
| `ci/helm-render/README.md` | Checks, fixture table, exit codes, read-only rule, the hazards in § 10 |
| `charts/paigasus/README.md` | The test list names four of the six scripts; name all six |

Plus the registration edits in § 7. `helm_render.py` falls under `repo:ruff-ci`
(`moon.yml:916`), so it must be ruff-clean. CODEOWNERS needs no change.

## 4. `run.sh`

### Setup

- `set -euo pipefail` at the top, then `export PROTO_REPORTER=text` (root CLAUDE.md standing rule,
  SMA-609).
- `REPO_ROOT` is computed from `BASH_SOURCE`, with no override.
- **`helm`.** Resolve it through proto to an absolute path. The model is
  `ci/release-parity/ecosystems/release-plz.sh:43-56`: change to `REPO_ROOT` first (the shim looks
  for `.prototools` from the current directory), pass `--reporter text`, and assert both `[ -f ]`
  and `[ -x ]`. Then assert that `helm version --short` starts with `v` plus the version pinned in
  `.prototools`. The golden files are a byte pin of that helm version. Any failure exits 2.
- **Python.** Resolve the venv interpreter ONCE:
  `uv run --locked --project "$REPO_ROOT/ci/helm-render" python3 -c 'import sys, yaml; print(sys.executable)'`.
  Assert that the result is an executable file under the project's `.venv`. Any failure exits 2.
  `--locked` stops `uv` from rewriting `uv.lock` in place. After this step, no `uv` runs. The
  module and the chart scripts run with `PATH="<venv bin>:<helm dir>:$PATH"`. So the chart
  scripts' bare `python3` gets PyYAML, and their bare `helm` gets the pinned helm. Never call
  `uv` from inside a fixture directory (`ci/ruff/README.md:150-179`, the PR 206 lesson).
- **Chart scripts run as `"$BASH" <script>`**, not through their `#!/usr/bin/env bash` line. So
  they run under the same bash as `run.sh`, and the bash 3.2 run in § 8 covers them.

### Exit codes

0 is pass, 1 is an assertion failure, 2 is an infrastructure error. They must not collapse into
each other.

- `helm_render.py` exits 0 on pass, **3** on an assertion failure and 2 on an infrastructure
  error (a render that fails, a source file it cannot parse). `run.sh` maps 3 to 1. Any other
  code from the module maps to 2. A Python traceback exits 1, so it can never read as an
  assertion failure. `ci/workflow-credentials/run.sh:31-40` is the model.
- A chart script that exits 1 is an assertion failure. Any other non-zero code (for example 127
  or 141) maps to 2.
- Capture every rc without `$( )` around a step that can `exit 2` (the "errexit swallows nested
  exit codes" trap).
- **Every row runs after a failure.** If the module exits 2 in the real run, `run.sh` still runs
  the six chart scripts. The gate's rc is the worst one seen: 2 before 1 before 0.

### Shell constraints

No `mapfile`, no `declare -A`, no `"${A[@]}"` on a possibly empty array without the `+` guard,
no pipe into an early-exit reader (`ci/actionlint` check 13). The gate must run under system
`/bin/bash` 3.2.57 and under bash 5.

### Read-only

The gate writes only under a `mktemp -d` directory, removed by a `trap`, with one exception: the
Python step creates `ci/helm-render/.venv`, which `.gitignore:32` ignores. It never writes into
`charts/`, into a build tree, or into any tracked file. It never calls `render.sh --update`.

### Modes

| Mode | Does |
| -- | -- |
| `--self-test` | Runs `helm_render.py --self-test` (in-process rows over inline YAML and proto text), plus wrapper rows: the module's 3 maps to 1, a chart-script rc of 127 maps to 2 |
| `--negative-control` | For each fixture: build the fixture chart, run the module against it, and check the result (below) |
| (none) | Runs checks 1–4 against `charts/paigasus`, then every chart script |

**The negative-control rule, per fixture.** The module must exit 3, and its output must contain
the fixture's own named row as a failure. Other rows may also fail. That is recorded, not an
error, unless the fixture says a named row must stay green (`leaked-value`, § 6). Per fixture:

- the expected result gives `OK`;
- rc 0, or rc 3 without the named row, gives `negative-control FAILED`;
- any other rc gives `negative-control INCONCLUSIVE: infrastructure error (rc=N)`.

**The total:** any FAILED gives 1. Otherwise any INCONCLUSIVE gives 2. Otherwise 0. The idiom of
`ci/release-parity/run.sh:66-75` is the model for one fixture.

## 5. Checks

**Renders.** Every render uses `helm template` with `--kube-version 1.31.0`, the value the chart
scripts use. The module keeps its own list of required stub values, as one constant. It does not
parse `render.sh`'s bash array. It is a separate list for one reason: it sets
`zones.gateway.backend.url` to the sentinel `http://gateway-sentinel.example.test:8088`, not to
the chart scripts' `http://gw.example.test:8088`. The golden files do not change.

**Valid subsets:** `iam` only, and `iam` plus `gateway`. Every check below runs over both,
unless it says otherwise.

### Check 1 — coupling and routing (AC 2)

For each subset, with `E` the subset's enabled zone ids:

1. The zone ids behind the rendered `Ingress` path prefixes (mapped through each zone's
   `basePath`), the keys of `PAIGASUS_ZONES`, and the keys of `PAIGASUS_SERVICES` are **each
   equal to `E`**. Equal to each other is not enough: a zone missing from all three would pass.
2. Each `Ingress` path routes to the console `Service` of the zone that owns that `basePath`, and
   that Service selects the console Deployment whose `PAIGASUS_ZONE` env value is that zone id. A
   rule that sends `/gateway` to the IAM console fails this row.
3. Every id in `E` is a member of the proto-derived slug set of check 1a.

### Check 1a — the slug mirror

The module derives the slug set from `contracts/proto/paigasus/common/v1/service_info.proto`:

- Parse only lines of the form `NAME = N;` (with optional options before `;`) inside the
  `enum Capability { … }` block. Comments that name an enum value (line 48) do not count.
- Skip `CAPABILITY_UNSPECIFIED`. Remove the `CAPABILITY_` prefix, change to lowercase, change
  `_` to `.`. This is `capabilityWireKey` in `ts/packages/paigasus-proto/src/capability.ts:21-27`.
- The slug is the part before the first `.`. **A key with no `.` fails closed** (rc 2 with a
  message). `state.ts:21` uses `k.slice(0, k.indexOf('.'))`, which for such a key drops the last
  character, so Python's `split('.')[0]` would disagree with TS without notice.

The chart's set is the literal of `paigasus.serviceSlugs` in `_helpers.tpl`, split with
`split(" ")` exactly as `paigasus.validate` splits it (`_helpers.tpl:57`). The two sets must be
**equal** (A2).

**The TS derivation is pinned as text.** The module also asserts that the two derivation lines
exist, unchanged, as literal lines: the `SERVICE_SLUGS` line in
`ts/packages/paigasus-discovery/src/core/state.ts` and the `return name.slice(PREFIX.length)…`
line in `ts/packages/paigasus-proto/src/capability.ts`. If either line changes, check 1a fails
and the author must re-confirm the rule. Both files are task inputs.

### Check 2 — a disabled zone leaves no trace

For the `iam`-only subset, walk every string key and every string value of every rendered
document, at any depth. No string may contain, case-insensitively:

- the id `gateway`;
- any value the render set under `zones.gateway`, including the sentinel host
  `gateway-sentinel.example.test`.

This is a plain substring search, as `ingress.sh:54-58` does. The iam-only render passes it today,
because `ingress.yaml:33-37` writes its comments per zone. A narrower token rule would only
remove coverage.

### Check 3 — independent rollout (AC 5)

The module renders twice per case, with both zones enabled unless the case says otherwise, and
compares `spec.template` of each Deployment as parsed YAML. Deployment-level labels and
annotations do not count. The three templates are: IAM console, gateway console, IAM backend.

| Case | Change | Required result |
| -- | -- | -- |
| a | `zones.iam.console.image.tag` from `t1` to `t2` | The IAM console template differs. The gateway console and the IAM backend templates are equal |
| a′ | `zones.gateway.console.image.tag` from `t1` to `t2` | The gateway console template differs. The IAM console and the IAM backend templates are equal |
| b | `appVersion` in a temp copy of `Chart.yaml`, all tags left at default | All three templates differ. A documented consequence, not a failure: the IAM backend image also defaults to `appVersion` (`backend-deployment.yaml:61`) |
| c | Disable `gateway` | The gateway console Deployment is absent. The IAM console template differs (`checksum/zonemap` moved, `console-deployment.yaml:26`). The IAM backend template is equal |

Each "differs" is a positive assertion. Case c's backend row is the strongest AC 5 property: a
zone-map change must not restart the backend. Case b edits `Chart.yaml` only in the temp copy.

### Check 4 — the chart agrees with itself

The F9 failure is a port mismatch inside the chart. The chart binds IAM explicitly through
`IAM_HTTP_ADDR` and `IAM_GRPC_ADDR` (`backend-deployment.yaml:73-79`), so the Rust binary's
default port never reaches the pod. The check therefore compares the chart with itself, not with
`config.rs`. Parent § 7.2's pin against the Rust files is withdrawn.

- **IAM HTTP:** the `http` containerPort, the port in `IAM_HTTP_ADDR`, the IAM backend Service's
  `http` port, and the port in `PAIGASUS_SERVICES.iam` are equal.
- **IAM gRPC:** the `grpc` containerPort, the port in `IAM_GRPC_ADDR`, the IAM backend Service's
  `grpc` port, and the port in `PAIGASUS_IAM_GRPC_URL` are equal.
- **Each console:** the containerPort, `int` of the env value `PORT`, and the console Service's
  target port are equal.
- **Security context, every Deployment:**
  - Pod level MUST carry `runAsUser: 65532`, `runAsGroup: 65532`, `runAsNonRoot: true`.
  - Container level MUST carry `allowPrivilegeEscalation: false`. It MUST NOT set `runAsUser`,
    `runAsGroup` or `runAsNonRoot` to a different value.
  - Neither level may carry `readOnlyRootFilesystem`. This pin follows RUNBOOK-containers.md
    § 7, which calls that posture untested. The README says that a hardening PR must change this
    row and that runbook section together.

### Check 5 — lint and golden files

The gate calls `charts/paigasus/tests/render.sh` with no `--update`, as the PR 2a plan intended
(plan line 1715).

### Check 6 — every chart script

The gate runs every `charts/paigasus/tests/*.sh`, each with `--set ingress.host=console.example.test`.
Only `refusals.sh` needs it (`charts/paigasus/README.md:117-126`). The others append `"$@"` to
their own values, so it does no harm. The list comes from the glob, sorted, with a floor: fewer
than six scripts is rc 2. A seventh script is then picked up without an edit. One row per script.
`render.sh` runs in this loop, so check 5 is one of these rows.

## 6. Negative-control fixtures

Each fixture is a directory under `ci/helm-render/fixtures/` whose tree mirrors
`charts/paigasus/`. It holds only the files it changes. The fixture chart is built as
`cp -R charts/paigasus "$tmp/paigasus"`, then `cp -R "$fixture/." "$tmp/paigasus/"`. The `/.`
form behaves the same under BSD and GNU `cp`. No file is edited in place, and no `sed -i.bak` is
used.

| Fixture | Mutation | Named row that must fail | Must stay green |
| -- | -- | -- | -- |
| `literal-ingress` | `ingress.yaml` writes both rules literally instead of ranging over enabled zones | check 1.1, `iam` only | — |
| `zones-omits-enabled` | `PAIGASUS_ZONES` skips one enabled zone | check 1.1 | — |
| `leaked-value` | A `console-env` data entry carries `zones.gateway.backend.url` when `gateway` is disabled. The zone-map key sets stay equal | check 2 | check 1.1 |
| `template-only-diff` | Every console Deployment takes its image tag from `zones.iam` | check 3 case a′ (bumping the gateway tag changes nothing) and case a (bumping the IAM tag changes both) | — |
| `slug-mirror` | `paigasus.serviceSlugs` lists one extra slug | check 1a | — |
| `security-context` | the console pod drops `runAsNonRoot` | check 4, security context | — |

`leaked-value` replaces parent § 8.2's `orphan-service`. That fixture put a `gateway` KEY into
`PAIGASUS_SERVICES`, so check 1 also caught it, and it proved nothing about check 2. Parent
§ 8.1 justifies check 2 by a VALUE written outside the `range`. `leaked-value` is that case, and
its "must stay green" column proves that check 1 cannot see it.

`template-only-diff` takes the tag from `zones.iam`, not from the first zone in range order.
Helm ranges alphabetically, so the first zone is `gateway`.

`ci/helm-render/README.md` records which fixture was measured against which row, as
`ci/release-parity/README.md` does.

## 7. Registration — ten obligations

**Gate registration: seven,** from the `repo:ruff-ci` entry in `ci/CLAUDE.md`. The plan re-reads
that file before it writes these, and follows any newer rule there.

1. `.github/workflows/ci.yml`'s `T=(…)` array: add `:helm-render`.
2. The marker-delimited command in the **root** `CLAUDE.md`: add `:helm-render`, byte-identical to
   `T`. Do not quote or copy the markers anywhere.
3. `SELF_SCHEDULED_GATES` in `ci/affected-graph/ci_targets.py`: the four `moon.yml` lines.
4. `SELF_TASK_EXPECTED_GLOBS`: the task's literal `inputs`. Note that the comparison sorts globs
   first, then files (`ci_targets.py:214-221`).
5. `HELM_RENDER_SH_CALL_SITES`, as discrete lines, each required to occur exactly once in
   `run.sh`: the flag parse, the `NEGATIVE` guard, the negative-control assertion body, both
   report arms, the self-test dispatch and its failure guard, the real-run module call, and the
   chart-script loop (glob, floor and invocation). The real-run lines are pinned because a
   deleted chart-script loop would otherwise leave the gate green with no lint and no golden
   diff. **This is more than one tuple.** `check_self_invocation` takes one positional text per
   gate (`ci_targets.py:1691-1695`). A new gate needs a `read_input` call in `main()`, a new
   parameter at every existing call, and the self-test rows the other gates have: a deletion row
   per pinned line, a contamination row, a commented-out row and a disabled-guard row
   (`ci_targets.py:3132-3182`).
6. `REQUIRED_REPO_TASKS`: because the gate carries a `--negative-control`.
7. `repo:affected-smoke`'s own `inputs` in `moon.yml`: add `ci/helm-render/**/*`, floored by an
   entry in `T_AFFECTED_SMOKE_REQUIRED_INPUTS` in `ci/actionlint/run.sh`.

**The new uv project: three,** the same as `ci/workflow-credentials/` (SMA-593):

8. `LOCKFILES` in `ci/osv/run.sh:33-37`: add `ci/helm-render/uv.lock`.
9. `repo:osv`'s `inputs` in `moon.yml` (`moon.yml:46-49`): add the same file.
10. A Dependabot `uv` block for `/ci/helm-render` in `.github/dependabot.yml`, shaped like the
    `/ci/workflow-credentials` block (`.github/dependabot.yml:73-94`).

**The Moon task.** `repo:helm-render` in the root `moon.yml`, `toolchain: 'system'`, with the
four-line script of obligation 3. Its `inputs`:

- `.prototools` and `.proto/plugins/helm.toml` (the helm pin; without them a helm bump does not
  select the gate, and CI serves a cached pass)
- `charts/**/*`
- `ci/helm-render/**/*`
- `contracts/proto/paigasus/common/v1/service_info.proto`
- `ts/packages/paigasus-discovery/src/core/state.ts`
- `ts/packages/paigasus-proto/src/capability.ts`

It has no `deps`: nothing in the gate reads a build output (parent § 8.4). The two `config.rs`
files are no longer inputs (check 4).

`EXPECTED_FINDING_KEYS` does not change: `check_self_invocation` is an existing check, and this
PR adds no key to `collect_findings`.

## 8. Verification

- `--self-test`, `--negative-control` and the real run each pass under `/bin/bash` 3.2.57 and
  under `/opt/homebrew/bin/bash`. The bash 5 run is subject to the 512-byte pipe condition in root
  CLAUDE.md. Record the pipe state if it hangs.
- Each fixture fails its own named row, measured one at a time. `leaked-value` also shows check
  1.1 green. The README records the result.
- **Delete-the-feature proof** for the rows no fixture covers: check 1.2 (routing), check 3 case b
  and case c, and the three port rows of check 4. Remove each assertion body and show that a
  self-test row goes red. A red-first test proves only that a test fails before the code exists.
- The wrapper self-test rows: the module's rc 3 maps to 1, a chart-script rc of 127 maps to 2.
- `moon run repo:helm-render` passes. The full gate graph in root CLAUDE.md passes, per its bash
  split rules.

## 9. Out of scope

- PR 3: `chart.yml`, `EXPECTED_PR_SUBJECTS`, `docs/ops/RUNBOOK-chart.md`, and the stale "five" in
  `ci/CLAUDE.md`'s `EXPECTED_PR_SUBJECTS` text.
- Changes to the chart templates and to the chart scripts. If a check finds a real defect in the
  chart, stop and report it. Do not change the golden files to make a check pass.
- A third console zone. Parent § 12 risk 1 stays open. A2 narrows it.

## 10. Residual risks

1. **Checks 1 and 2 exist twice** (A4). `ingress.sh` keeps its weaker copy.
2. **Check 1a reads the proto text, not the generated TS.** The text pin on the two TS
   derivation lines catches a change to those lines. It does not catch a change elsewhere, for
   example in `PREFIX`.
3. **Overlay fixtures copy whole files.** A template refactor can make a fixture stale. The
   negative control then reports FAILED or INCONCLUSIVE, and someone must refresh the fixture by
   hand.
4. **The module's stub list is a seventh copy** of the required values (`render.sh:21-34` and the
   other five scripts hold the rest). A new required value must be added to all seven. A missing
   one makes the module's render fail, which exits 2, so the error is loud.
5. **`maps.sh` reads only part of its input in a pipe under `pipefail`** (`maps.sh:37-46`, a
   `break` after the zone-map document). On the development Mac in the 512-byte pipe state, its
   `printf` can get SIGPIPE (141), which the gate reports as rc 2. On a Linux runner the render
   (about 14–20 KB) fits in the 64 KiB pipe. Not measured. The README records it. This PR does not
   change the chart scripts.

## 11. What stays from parent § 8

Parent § 8.4's affectedness reasoning, its read-only rule and its shell constraints stay in force.
Parent § 8.1 check 5 and § 8.2's first, second and fourth fixtures stay, as described here.

## 12. What the adversarial challenge changed

Verdict on revision 1: **APPROVE WITH CHANGES**, 1 blocker, 7 major. I checked the port binding,
the security-context shape, the TS zero-value skip and the `LOCKFILES` list against the tree. All
four were as the challenge said.

| Finding | Change |
| -- | -- |
| BLOCKER: `uv run` around every step mixes rc 1 and rc 2, and can re-lock | § 4: resolve the venv once with `--locked`; the module exits 3; rc mapping; wrapper self-test rows |
| `.prototools` missing from inputs | § 7 inputs; § 4 helm version assert |
| A4's reason was false | § 2 A4 states the real reasons |
| Check 2 misses the URL leak; `orphan-service` does not isolate check 2 | § 5 check 2 substring search with a sentinel; § 6 `leaked-value` replaces `orphan-service` |
| Real-run wiring not pinned | § 7 obligation 5 pins it; § 5 check 6 uses a glob with a floor |
| The uv project brings three obligations | § 7 obligations 8–10 |
| Port pin tested the wrong property | § 5 check 4 compares the chart with itself; `config.rs` inputs removed |
| Security-context rule ambiguous | § 5 check 4 exact rule; § 6 `security-context` fixture |
| Minors | check 3 exact table; check 1a parsing and no-dot fail-closed and text pin; check 1 expected set; `maps.sh` hazard recorded; helm model citation; check 6 wording; `"$BASH"` invocation; negative-control total; portable `cp`; § 8 wording; `.venv` exception; obligation 5 scope |
| Questions | A2 equality with a reason; `readOnlyRootFilesystem` pin tied to the runbook; rc 2 does not stop the battery; stub list is a stated residual; routing added as check 1.2 |

**Not folded in:** storing fixtures as unified diffs applied with `patch --forward`. It would make
a stale fixture fail loudly. It was not adopted, because the overlay form was an explicit choice
in the brainstorm (A3), and the negative control already reports a stale fixture.
