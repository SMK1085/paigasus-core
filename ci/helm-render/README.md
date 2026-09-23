<!-- SPDX-License-Identifier: Apache-2.0 -->

# `repo:helm-render`

The static gate over `charts/paigasus` (SMA-513 PR 2b). It renders the chart with the pinned
`helm` and asserts properties that `helm lint` and the golden files cannot see. It also runs
every script under `charts/paigasus/tests/`, which nothing else runs in CI.

Spec: `docs/superpowers/specs/2026-09-22-sma-513-pr2b-helm-render-gate-design.md`, which
replaces § 8 of `docs/superpowers/specs/2026-09-19-sma-513-multi-zone-ingress-helm-design.md`.

## Modes

| Command | Does |
| -- | -- |
| `bash ci/helm-render/run.sh` | Checks 1–4 (`helm_render.py --chart charts/paigasus`), then every `charts/paigasus/tests/*.sh` (checks 5 and 6) |
| `bash ci/helm-render/run.sh --self-test` | `helm_render.py --self-test` (in-process rows over inline YAML and proto text), then the wrapper's rc rows |
| `bash ci/helm-render/run.sh --negative-control` | Each fixture under `fixtures/` must make the module exit 3 with its own named row failed |

`moon run repo:helm-render` runs all three, the two controls first, under `set -euo pipefail`.

## Checks

Every render is `helm template paigasus <chart> --kube-version 1.31.0` with the stub values in
`helm_render.py`'s `STUB_VALUES`. The valid subsets are `iam` and `iam+gateway`.

| Row | Check | It fails when |
| -- | -- | -- |
| `1a` | The slug mirror | `paigasus.serviceSlugs` in `_helpers.tpl` is not EQUAL to the slug set derived from `enum Capability` in `service_info.proto`, or a TypeScript derivation line changed |
| `1.1 <subset>` | Coupling (AC 2) | The Ingress zone ids, the `PAIGASUS_ZONES` keys or the `PAIGASUS_SERVICES` keys are not each EQUAL to the enabled set |
| `1.2 <subset>` | Routing | An Ingress path routes to a Service whose Deployment serves another `PAIGASUS_ZONE` |
| `1.3 <subset>` | Membership | An enabled zone id is not in the proto-derived slug set |
| `2` | No trace of a disabled zone | Any string key or value of the iam-only render, or its raw text, contains `gateway`, the sentinel host or a value set under `zones.gateway` (case-insensitive) |
| `3a`, `3a-prime`, `3b`, `3c` | Independent rollout (AC 5) | `spec.template` of a Deployment differs, stays equal or survives against the table in spec § 5 check 3 |
| `4 iam-http <subset>` | Ports agree | The IAM `http` containerPort, `IAM_HTTP_ADDR`, the Service's `http` port and target, and `PAIGASUS_SERVICES.iam` disagree |
| `4 iam-grpc <subset>` | Ports agree | The IAM `grpc` containerPort, `IAM_GRPC_ADDR`, the Service's `grpc` port and target, and `PAIGASUS_IAM_GRPC_URL` disagree |
| `4 console-port <subset>` | Ports agree | A console's containerPort, `PORT` and its Service's resolved target port disagree |
| `4 security-context <subset>` | Security context | A pod lacks `runAsUser: 65532`, `runAsGroup: 65532`, `runAsNonRoot: true`; a container lacks `allowPrivilegeEscalation: false` or sets an identity key to another value; either level carries `readOnlyRootFilesystem` |
| `6 <script>` | Every chart script | A `charts/paigasus/tests/*.sh` exits non-zero. `render.sh` is check 5 (lint and golden files) |

**Why equality for the slug mirror (spec A2).** The safety property is "chart ⊆ proto".
Equality is stronger on purpose: the contracts PR that registers the first capability of a new
service must also edit `charts/`. That is the only moment anyone thinks about a new zone.

**The `readOnlyRootFilesystem` pin.** `docs/ops/RUNBOOK-containers.md` § 7 says that posture is
untested for every image in this repo. A hardening PR must change this row and that runbook
section together.

## Exit codes

| Code | Meaning |
| -- | -- |
| 0 | pass |
| 1 | an assertion failed (the module's 3, or a chart script's 1) |
| 2 | infrastructure error (a failed render, an unparseable source, a missing tool, a chart script's 127 or 141, fewer than seven chart scripts) |

`helm_render.py` exits 3, not 1, for an assertion, because a Python traceback exits 1. Do not
"normalize" it. Every row runs after a failure; the gate's rc is the worst one seen.

## The negative control

Each fixture holds only the files it changes. `run.sh` builds the fixture chart as
`cp -R charts/paigasus <tmp>/paigasus`, then `cp -R <fixture>/. <tmp>/paigasus/`. Per fixture,
the module must exit 3 with the named row failed. Other rows may also fail. Measured one fixture
at a time on 2026-09-22:

| Fixture | Mutation | Named row | Must stay green | Rows that failed |
| -- | -- | -- | -- | -- |
| `literal-ingress` | both Ingress rules written literally | `1.1 iam` | — | `1.1 iam`, `1.2 iam`, `2` |
| `zones-omits-enabled` | `PAIGASUS_ZONES` skips an enabled `gateway` | `1.1 iam+gateway` | — | `1.1 iam+gateway` |
| `leaked-value` | a `console-env` entry carries `zones.gateway.backend.url` | `2` | `1.1 …` | `2` |
| `template-only-diff` | every console reads `zones.iam`'s image tag | `3a`, `3a-prime` | — | `3a`, `3a-prime` |
| `slug-mirror` | `paigasus.serviceSlugs` lists `billing` too | `1a` | — | `1a` |
| `security-context` | the console pod drops `runAsNonRoot` | `4 security-context iam`, `4 security-context iam+gateway` | — | the two named rows |

`literal-ingress` also fails `1.2 iam` and `2` by construction: a literal `/gateway` rule both
names the disabled zone and routes to a Service that does not exist.

Verdicts: the expected result is `OK`; rc 0, or rc 3 without a named row, is
`negative-control FAILED`; any other rc is `negative-control INCONCLUSIVE: infrastructure error
(rc=N)`. Any FAILED gives 1, else any INCONCLUSIVE gives 2, else 0. Measured: deleting check 2's
body makes `leaked-value` report FAILED (rc 1); a table row with no fixture directory reports
INCONCLUSIVE (rc 2).

**Refreshing a stale fixture.** A fixture is a whole-file copy of one live template. A template
refactor can make it stale, and the control then reports FAILED or INCONCLUSIVE. Rebuild it from
the live file: copy the file, then re-apply the one mutation in the table above.

## Delete-the-feature record (spec § 8)

For the rows no fixture covers, each assertion body was removed in turn and the module
self-test went red (rc 3): check 1.2 routing (2 rows red), check 3 case b (1), check 3 case c
(3), `4 iam-http` (3), `4 iam-grpc` (2), `4 console-port` (3). The wrapper's rc maps were broken
in turn (3 to 3, any to 1, any to 0) and `--self-test` exited 1 each time. Deleting any one of
the 35 `HELM_RENDER_SH_CALL_SITES` lines from `run.sh` makes `ci_targets.py` report it.

## Tool resolution

- `helm` resolves once through `proto --reporter text bin helm` from the repo root, and must be
  an executable file whose `helm version --short` starts with `v` plus the `.prototools` pin.
  No fallback: the golden files are a byte pin of that helm.
- Python resolves once through `uv run --locked --project ci/helm-render`, and must be a file
  under `ci/helm-render/.venv`. After that no `uv` runs.
- The module and the chart scripts run with the venv's `bin` and the helm directory first on
  `PATH`. Chart scripts run as `"$BASH" <script>`, so the bash that runs `run.sh` runs them too.

## Read-only rule

The gate writes only under one `mktemp -d` directory, removed by a `trap`. `TMPDIR` and the
`HELM_*_HOME` variables point into it, so check 3 case b's `Chart.yaml` copy lands there. The one
exception is `ci/helm-render/.venv`, which `.gitignore` ignores. The gate never writes into
`charts/`, never calls `render.sh --update`, and never runs `uv` inside a fixture directory.
Nothing in the repository enforces this rule; review does.

## Residual risks (spec § 10)

1. Checks 1 and 2 exist twice. `charts/paigasus/tests/ingress.sh` keeps its weaker copy.
2. Check 1a reads the proto text, not the generated TypeScript. The text pin on the two
   derivation lines catches a change to those lines, not a change elsewhere (for example to
   `PREFIX` in `capability.ts`).
3. Overlay fixtures copy whole files, so a template refactor can make one stale (above).
4. `STUB_VALUES` is an eighth copy of the required values. A new required value must go into all
   eight, and into `ci/kind/values/a.yaml` (row 7); a missing one makes every render fail, which is rc 2.
5. `maps.sh` reads only part of its input in a pipe under `pipefail` (`maps.sh:37-46`). On a
   host whose new pipe holds 512 bytes, its `printf` can get SIGPIPE (141), which this gate
   reports as rc 2. A Linux runner's 64 KiB pipe holds the whole render. Not measured. This PR
   does not change the chart scripts.

## Running it locally

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/helm-render/run.sh --self-test
bash ci/helm-render/run.sh --negative-control
bash ci/helm-render/run.sh
```

It runs under `/bin/bash` 3.2.57 and under bash 5: it uses no `mapfile`, no `declare -A`, no
here-string and no pipe into an early-exit reader. Measured 2026-09-22 under both, with a host
pipe capacity of 65536 bytes: about 2 s per mode.
