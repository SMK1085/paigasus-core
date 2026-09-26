# SMA-691 measurements: the audience warning and its mutation battery

Measured on 2026-09-26 in the worktree of branch `feature/sma-691-chart-default-audience`, with Helm
v3.22.0+g144ca65, `/bin/bash` 3.2.57 and Python 3.14.7 with PyYAML 6.0.3. The spec
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
| M1 | drop condition 1 (audience equals client id) | W4, N3 | W4, N3 (exact match) |
| M2 | drop condition 2 (the acknowledgement) | W5, N4 | W5, N4, plus extra: W10, W14 |
| M3 | compare the acknowledgement with `"true"` | W5, W6 | W5, W6, N4, plus extra: W10, W14 |
| M4 | the annotation in `spec.template.metadata.annotations` | W1, W9 | W1, W2, W3, W6, W7, W8, W9, W11, W12, W13, W14 (exact match); render.sh `FAIL [iam-only]` and `FAIL [iam-and-gateway]` |
| M5 | drop `eq $id "iam"` | W1 (count) | NONE (equivalent mutant, confirmed); render.sh green |
| M6 | `NOTES.txt` without the include | N0 | N0, N1, N2, N5 (exact match) |
| M7 | change the marker line text | N1 | N1, N2, N5 (exact match) |
| M8a | delete the W3 call line | the row counter | `FAIL [warning rows]: 13 warning row(s) ran, want 14` |
| M8b | delete the N5 call line | the row counter | `FAIL [notes rows]: 5 notes row(s) ran, want 6` |
| M9a | drop `toString` in `paigasus.iamAudience` | A4, A5 | NONE (equivalent mutant, confirmed) |
| M9b | drop `default` in `paigasus.iamAudience` | (replaces M9a) | A1, A2, W1, W2, W6, W7, W8, N1, N5, plus extra: W11, W12, W13, W14 |
| M10 | the annotation's Go comment without left trim | (plan) | env.sh green; render.sh `FAIL [iam-only]` and `FAIL [iam-and-gateway]` (exact match) |
| M11 | the NOTES include without trim | (plan) | N0 only (exact match) |
| M12 | a case-insensitive compare | (plan) | W12 (exact match) |
| M13 | the compare without `toString` | (plan) | W6, W10, W11, each `render failed`: incompatible types for comparison (exact match) |
| M14 | the acknowledgement fed into a pod checksum | (plan) | W14: `iam-backend: spec.template differs; it must be equal` (exact match) |
| M15 | delete the `upgrade b` NOTES check in `ci/kind/run.sh` | nothing (R2) | no committed check reds; the scratch harness reds `FAIL [call site upgrade b]: 0 line(s), want 1` (exact match) |
| M16 | replace `{{ include "paigasus.iamAudience" . \| quote }}` in `_audience.tpl` with the literal `"paigasus-console"` (the N6 test gap fix) | N6 | N6 (exact match); `/bin/bash` and `/opt/homebrew/bin/bash` both rc 0 after the inverse edit |

## Two equivalent mutants

- **M5.** `paigasus.validate` refuses the gateway backend while the gateway zone is on, so every
  successful render has exactly one backend Deployment. Without `eq $id "iam"`, the render does
  not change. No row can fail. The spec expected W1 to fail. `eq $id "iam"` stays as protection
  for a future second backend.
- **M9a.** `include` renders the helper to text, and the template printer writes a number as its
  digits. So `toString` in `paigasus.iamAudience` changes no render. The spec expected A4 and A5
  to fail. That was true for the inline expression before the refactor. M9b is the mutation that
  proves A1 and A2 read through the helper.

## Rows that reded more than the spec expected

M2, M3 and M9b each reded extra rows beyond the spec's "Expected red rows" column. Each of the
three still reded every row the spec expected, so none of them is a weaker mutant than the spec
assumed. No mutation reded fewer rows than expected, so no row failed to bite.

- M2 (drop the acknowledgement condition) also reded W10 and W14, both of which exercise the
  acknowledgement value directly.
- M3 (compare the acknowledgement with `"true"`) also reded W10 and W14, the same two rows as M2.
- M9b (drop `default` in `paigasus.iamAudience`) also reded W11, W12 and W13. The exact mechanism
  for these three was not traced further; the row is the measured fact, not an explained one.
