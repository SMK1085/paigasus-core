# SMA-678: chart value `oidc.audience`

- Linear: SMA-678 (found during SMA-513 PR 3, spec § 11)
- Status: draft for approval, revised after the spec challenge (§ 7)
- Path: bounded change, written as a short spec for the feature-pipeline challenge step

## 1. Problem

The chart makes IAM's issuer configuration from two values
(`charts/paigasus/templates/backend-deployment.yaml:100-101`):

```yaml
- name: IAM_AUTHN__ISSUERS
  value: {{ printf "[{issuer=%q,audiences=[%q]}]" $root.Values.oidc.issuer $root.Values.oidc.clientId | quote }}
```

IAM validates the ACCESS token. It accepts the token only if the `aud` claim contains one of the
configured audiences (`rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs:198`,
`jsonwebtoken` `Validation::set_audience`, any-of match). So today the access token's `aud` must
contain `oidc.clientId`.

Some IdPs put a different value in `aud`. Okta's default authorization server uses
`api://default`. An operator with such an IdP cannot use the chart without a change on the IdP.

## 2. Decisions

| # | Decision | Reason |
|---|---|---|
| D1 | Add one optional string value, `oidc.audience`. Its default in `values.yaml` is `""`. | The issue asks for one value. IAM accepts a list, but a list value is not necessary for the known case (YAGNI). |
| D2 | When `oidc.audience` is not empty, it REPLACES `oidc.clientId` in `audiences`. It is not added next to it. | Sven chose this on 2026-09-24. The ID token has `aud` = the client id. The validator does not check the token type (`validator.rs:196-200`), so TODAY IAM accepts an ID token as a bearer token. With a replacement, IAM refuses an ID token when `oidc.audience` is set. The add semantics would keep that gap open in every configuration. The default configuration keeps the gap (§ 5). |
| D3 | When `oidc.audience` is empty or absent, the chart uses `oidc.clientId`, as today. | AC 1: the default render stays byte-identical. The golden files do not change. |
| D4 | The value is OPTIONAL. It does not go into the eight stub-value copies or into `ci/kind/values/a.yaml`. | `ci/helm-render/README.md` risk 4 and `charts/CLAUDE.md` apply to a REQUIRED value only. A missing optional value does not make a render fail. |
| D5 | Read the value as `$root.Values.oidc.audience`, with no `dig`. | `dig` is necessary for a value under an optional parent map (`oidc.caBundle.*`). `oidc` is always present, because `oidc.issuer` and `oidc.clientId` are required. Under `--reuse-values`, a release made before this change has no `audience` key. The template then reads nil, and `default` gives `oidc.clientId`. Row A2 renders this nil case. |
| D6 | Do not change the YAML comment lines 97-99 above `IAM_AUTHN__ISSUERS`. Put the design reason in a Go template comment (`{{- /* … */}}`, left-trim only), which does not render. When `oidc.audience` is set, render one extra YAML comment line inside `{{- if $root.Values.oidc.audience }}`. | A YAML comment in a template renders into the manifest and into the golden files (`charts/CLAUDE.md`). A changed unconditional comment breaks AC 1. Lines 97-99 say that the client id is the audience. The conditional line corrects that statement in the manifest when it is not true. |
| D7 | Convert the value to a string with `toString` before `%q`. | `--set oidc.audience=12345` gives an int64, and `%q` on an integer writes a rune literal. IAM then cannot parse the issuer list and does not boot. `toString` does not change a string, so AC 1 holds. |
| D8 | No change to IAM's production code, to the console, or to the kind job. Add one IAM unit test only (§ 3.5). | IAM already accepts any list of audiences. The console sends only `scope`, with no `audience` or `resource` parameter (`oidc.ts:223`). The gateway has no audience configuration; it introspects through IAM. The kind job uses Keycloak with an audience mapper for the client id, so it keeps the default. |
| D9 | Put the test rows in the existing `charts/paigasus/tests/env.sh`, in a separate function. Do not add a new `tests/*.sh` file. | By convention, a new chart script raises `CHART_SCRIPT_FLOOR` in `ci/helm-render/run.sh:33`, which `ci/affected-graph/ci_targets.py:1409` pins, plus the prose counts in the README and `helm_render.py`. That is gate machinery for a small test. `env.sh` already checks the environment keys that reach containers. |

## 3. Changes

### 3.1 `charts/paigasus/values.yaml`

Add, under `oidc:`, after `clientId`:

```yaml
  audience: ""           # NOT required. The access-token audience that IAM accepts. Empty means
                         # oidc.clientId. Set it only when your IdP puts a different value in the
                         # access token's `aud` claim (for example Okta: api://default). It REPLACES
                         # oidc.clientId; IAM then does not accept oidc.clientId as the audience.
                         # Quote it in a values file. See docs/ops/RUNBOOK-chart.md § 6.
```

### 3.2 `charts/paigasus/templates/backend-deployment.yaml`

- Lines 97-100 stay byte-identical.
- Before line 100, add a left-trim-only Go template comment (`{{- /* … */}}`) on its own line. It
  states D2, D3 and D7. It must not trim on the right: `-}}` would join the next line.
- Before line 100, add a block `{{- if $root.Values.oidc.audience }}` with one YAML comment line:
  `# oidc.audience is set: IAM accepts that audience, not the client id.` The comment is inside
  the block, so the default render does not change.
- Line 101: the second `%q` argument becomes
  `($root.Values.oidc.audience | default $root.Values.oidc.clientId | toString)`.

### 3.3 `charts/paigasus/tests/env.sh`

Update the header comment. It must say that the script also checks the IAM backend's
`IAM_AUTHN__ISSUERS` value (SMA-678), and not only the console keys.

Add a function `check_audience <label> <expected-audience> [helm args…]`. It renders the chart
with `BASE` plus the arguments. In the inline Python block, it:

- selects the Deployment with label `app.kubernetes.io/name=iam-backend`, as
  `ca-bundle.sh:79` does, and asserts that exactly one such Deployment exists;
- asserts that exactly one `IAM_AUTHN__ISSUERS` entry exists in its container env;
- compares the value with the EXACT string
  `[{issuer="https://idp.example.test/realms/paigasus",audiences=["<expected>"]}]`. An exact
  compare also proves that the list holds one element only (D2).

Each call increments a counter. After the rows, the script fails if the counter is less than the
number of rows (4). So a deleted call line goes red.

The script must stay safe under bash 3.2: no `mapfile`, no `declare -A`, no here-string, and no
pipe into a reader that exits early (`ci/helm-render/README.md:151`). The Python block uses double
quotes only, as the existing block does.

Rows:

| Row | Arguments | Expected `audiences` | Proves |
|---|---|---|---|
| A1 unset | (none) | `["paigasus-console"]` | AC 1, value-level |
| A2 reuse-values-no-key | `--set oidc.audience=null` | `["paigasus-console"]` | D5: the nil path, as for a release made before the key existed. The same pattern as `ca-bundle.sh` `reuse-values-no-key`. |
| A3 set | `--set oidc.audience=api://default` | `["api://default"]` | AC 2, and replacement (D2) |
| A4 number | `--set oidc.audience=12345` | `["12345"]` | D7 |

A3 also asserts that the conditional comment line from § 3.2 is present in the rendered manifest,
and A1 asserts that it is absent. The unchanged golden files (`charts/paigasus/tests/render.sh`)
prove AC 1 byte-for-byte.

**Proof that the rows bite.** Before commit, apply each mutation, run `env.sh`, and confirm the
named row goes red:

| Mutation | Red row |
|---|---|
| Line 101 uses `oidc.clientId` again | A3 |
| Remove `default` (use `oidc.audience` bare) | A1, A2 |
| Remove `toString` | A4 |
| Add the audience next to the client id | A3 |
| Delete one `check_audience` call line | the row counter |

Revert each mutation with the Edit tool, not with `git checkout --`. `git checkout --` also
discards the uncommitted change under test.

### 3.4 `docs/ops/RUNBOOK-chart.md`

- **Values table.** Add a row: `oidc.audience` | no | "The access-token audience IAM accepts.
  Default: `oidc.clientId`. Set it only when the IdP puts another value in `aud` (§ 6)". Change
  the `oidc.clientId` row to: "IAM also uses it as the access-token audience, unless
  `oidc.audience` is set (§ 6)".
- **§ 5, "What restarts what".** Add a row: a change to `oidc.audience` restarts the IAM pod. IAM
  has one replica and `maxSurge: 0` (`backend-deployment.yaml:13-25`), so IAM is not available
  during the restart.
- **§ 6 item 1.** Replace the "future work" text. The access token's `aud` must contain the
  audience that IAM accepts. That is `oidc.audience` when it is set, and `oidc.clientId` when it
  is not. Set `oidc.audience` only when you cannot make the IdP put the client id into `aud`. The
  value replaces the client id; it is not added to it. Before you choose the value, decode a real
  access token and read its `aud` claim. The value helps only when the IdP issues a JWT access
  token for the console's scopes (`openid profile email offline_access`). The console sends no
  `audience` or `resource` parameter. So an IdP that then issues an opaque token, or a token for a
  different API, cannot work with this value. A wrong audience shows as a refused token in the IAM
  log (`ci/kind/README.md:46`).
- **§ 6 Okta note**, after the Keycloak example. Mark it: "Not tested against a live Okta
  tenant." Three steps:
  1. Set `oidc.issuer` to the authorization-server issuer
     (`https://<org>.okta.com/oauth2/default`, or your custom server). Do not use the org
     authorization server (`https://<org>.okta.com`): other parties must not validate its access
     tokens.
  2. Set `oidc.audience=api://default`, or the audience of your custom server.
  3. Add an `email` claim (value `user.email`, included in the access token) to that
     authorization server. IAM needs `email` (item 2).

  Also add a warning. In Okta's default configuration, the access token's `sub` can be the user's
  login, not the fixed user id. IAM keys the identity on the access token's `(iss, sub)`. If that
  is true for your tenant, a login rename makes a new identity. Check `sub` in a real access token
  before production use. This spec did not verify Okta's `sub` behaviour.

### 3.5 `rs/crates/services/paigasus-iam/src/config.rs` (test only)

Add one `figment::Jail` unit test. It sets `IAM_AUTHN__ISSUERS` to the exact string that row A3
renders and asserts `audiences == ["api://default"]`. It proves the contract between the chart
string and IAM's parser for a URI-shaped audience. Today the env-form tests use only
`audiences=["paigasus"]`. Follow the existing Jail tests in that file for the other required
settings.

### 3.6 `charts/paigasus/README.md`

Add a short section for `oidc.audience`, in the same style as the `oidc.caBundle` section
(lines 97-113): what it sets, the default, the replacement semantics, and the test rows in
`env.sh`.

### 3.7 Not changed

- `charts/paigasus/templates/_helpers.tpl`: no `fail` check (§ 5, residual R2).
- The golden files, the eight stub-value copies, `ci/kind/values/a.yaml`, and `charts/CLAUDE.md`.

## 4. Acceptance criteria

1. With `oidc.audience` unset, the render is byte-identical to the render on `main`. Proof:
   `bash charts/paigasus/tests/render.sh` passes with no golden-file change; rows A1 and A2 pass.
2. With `oidc.audience=api://default`, `IAM_AUTHN__ISSUERS` is
   `[{issuer="…",audiences=["api://default"]}]`, and IAM parses that string to
   `["api://default"]`. Proof: row A3 and the § 3.5 test.
3. The runbook states when to set the value, that it replaces the client id, and when it is not
   enough. Proof: § 3.4 text in `docs/ops/RUNBOOK-chart.md`.
4. Every mutation in § 3.3 turns its named row red.
5. `repo:helm-render` passes (`--self-test`, `--negative-control`, and the full run), and
   `paigasus-iam` tests pass.

## 5. Out of scope and residuals

- **R1: no end-to-end proof of a non-default audience.** The kind job keeps the default. A second
  Keycloak audience mapper plus `oidc.audience` in `ci/kind/values/b.yaml` could prove it. Not in
  this issue.
- **R2: a padded or whitespace-only value.** `default` treats `" "` as set. IAM checks only that
  the list is not empty (`config.rs:1047-1049`), so IAM boots and refuses every token. A
  `paigasus.validate` refusal needs a re-sync of the two `_helpers.tpl` fixtures under
  `ci/helm-render/fixtures/`. Not in this issue.
- **R3: IAM accepts an ID token as a bearer token in the default configuration** (D2). A check of
  RFC 9068 `typ: at+jwt` is a separate issue.
- **R4: a large number in a values file.** `toString` on a float64 can give an exponent form. The
  runbook and `values.yaml` tell operators to quote the value.
- **A list value (`oidc.audiences`).** If a later issue adds it, the chart must refuse a render
  that sets both keys. It must not choose one silently.
- A value that contains a double quote or a backslash. `%q` escapes it, but no known IdP uses one.
- An e2e test against a real Okta tenant.

## 6. Spec challenge (2026-09-24)

Verdict: APPROVE WITH CHANGES (0 BLOCKER, 2 MAJOR, 10 MINOR, 3 QUESTION).

Folded in: the nil row (A2), the complete Okta recipe with a `sub` warning, `toString` (D7) and row
A4, the conditional YAML comment (D6), the corrected D2 reason, the "not enough" runbook sentence,
the `env.sh` constraints, the corrected floor location (D9), the row counter, the Edit-based
revert, the IAM parse test (§ 3.5), the § 5 restart row, and the chart README section.

Not folded in, recorded as residuals: whitespace validation (R2), token-type confusion (R3), the
e2e proof (R1).
