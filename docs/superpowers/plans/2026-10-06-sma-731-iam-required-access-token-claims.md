# SMA-731 IAM Required Access-Token Claims Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An operator can make IAM refuse a verified token that does not carry a configured claim,
with one list for each issuer. The check fails closed. The default (an empty list) does not change
the behaviour for any IdP. For Zitadel the recipe is `["jti"]`, next to the SMA-703 marker claims.

**Architecture:** `IssuerConfig` gets `access_token_required_claims: Vec<String>` (default empty).
One shared private function in `config.rs` validates both claim lists at boot, and a new rule
refuses a name in both lists. In the OIDC validator a new private value object `ClaimRules` holds
both lists. It decides the decode (`StrictPayload` when either list is set) and returns the refusal
(step 6b marker, then step 6c missing claim). A new `RefusalDetail::MissingClaim` logs
`missing claim <name>` with its own message. The chart renders `oidc.accessTokenRequiredClaims`
through one parameterized validation template that both lists share. The runbook § 6 gets the
fail-closed paragraph and the Zitadel recipe.

**Tech Stack:** Rust 1.95 (edition 2024), `jsonwebtoken` 11.1.0, `serde_json`, `figment` 0.10.19,
`tracing`, `paigasus_logging::test_support`, `cargo nextest`, testcontainers (Keycloak 26.4,
Zitadel v4.15.3); Helm 3.22.0 (Sprig templates), bash 3.2 chart scripts with inline `python3`.

**Spec:** `docs/superpowers/specs/2026-10-06-sma-731-iam-required-access-token-claims-design.md`
(template: `docs/superpowers/specs/2026-10-02-sma-703-zitadel-id-token-marker-claims-design.md`,
measurements: `docs/superpowers/specs/2026-10-02-sma-703-zitadel-measurements.md`).

## Global Constraints

- Work only in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims`, on branch `feature/sma-731-iam-required-access-token-claims`. Do not `cd` to the main checkout.
- Put the proto shims first in every shell: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` and `export PROTO_REPORTER=text`.
- Config field: `IssuerConfig::access_token_required_claims: Vec<String>`, `#[serde(default)]`. Chart value: `oidc.accessTokenRequiredClaims`, default `[]`.
- The constant `RESERVED_MARKER_CLAIMS` becomes `RESERVED_CLAIM_NAMES` (`["iss", "sub", "aud", "exp"]`). One shared private function `validate_claim_names` validates both lists. The SMA-703 messages for `id_token_marker_claims` do not change.
- New boot rule: a name in both lists of one issuer fails `validate`, and the message names both fields.
- Names compare exactly (case-sensitive), in IAM and in the chart.
- Validator: a private `ClaimRules { id_token_markers: Vec<String>, required: Vec<String> }` with `needs_strict_decode(&self) -> bool` and `refusal(&self, members: &serde_json::Map<String, serde_json::Value>) -> Option<RefusalDetail<'_>>`. `ConfiguredIssuer` holds `claim_rules: ClaimRules`, not a second loose list.
- Order in `authenticate`: decode, step 6 (SMA-686 markers), step 6b (marker claims), step 6c (required claims, new), step 7 (key binding). It applies to both `TokenScheme::Bearer` and `TokenScheme::Dpop`.
- A required claim is present when the verified payload has a top-level member with that name whose value is not JSON `null`. The first configured name that is missing is the one in the log.
- Decode: `StrictPayload` when EITHER list is not empty; `decode::<WireClaims>` exactly as today when both are empty (G2).
- Defect: `TokenDefect::NotAnAccessToken`. No new `TokenDefect` variant, no new metric, no change to the HTTP or gRPC response (401, `invalid-token`).
- New `RefusalDetail::MissingClaim(&'a str)`. Log marker `missing claim <name>`. Log message, exact: `refused a bearer token: it does not carry a claim that the issuer configuration requires`. Level `info`. Rate-limit key unchanged: (issuer, `NotAnAccessToken`), shared with the other `NotAnAccessToken` refusals.
- Boot line (exact): `IAM refuses a verified token of this issuer that does not carry every configured required claim`, one `info` line for each issuer with a non-empty list, written in `OidcAuthenticator::new`.
- Chart: the default render stays byte-identical. Never run `render.sh --update`. The golden files (`charts/paigasus/tests/golden/*.yaml`) must not change.
- Chart: `oidc.accessTokenRequiredClaims` goes in `values.yaml` AFTER the `oidc.scopes` block (before `authorizationAudience`), not after `idTokenMarkerClaims` (the `env.sh` M8 `sed` range deletes from `idTokenMarkerClaims:` to `scopes:`).
- Chart: everything stays in `templates/_iam-backend.tpl`. Do not edit `templates/_helpers.tpl` (its whole-file copies live in `ci/helm-render/fixtures/`). No new required value, so `helm_render.py` `STUB_VALUES`, the chart scripts' required-value lists and `ci/kind/values/a.yaml` do not change.
- Chart scripts run under `/bin/bash` (3.2) with the worktree root as the current directory: `/bin/bash charts/paigasus/tests/<script>.sh --set ingress.host=console.example.test`. Check that `helm version --short` prints `v3.22.0+g144ca65`. No `mapfile`, no `declare -A`, no here-string.
- Rust checks, run from `rs/`: `cargo fmt --check -p paigasus-iam -p paigasus-iam-core`, `cargo clippy -p paigasus-iam -p paigasus-iam-core --all-targets --locked -- -D warnings`, `cargo nextest run -p paigasus-iam --lib --locked`.
- Docker suites: always set `PAIGASUS_REQUIRE_DOCKER=1`, so a skip panics: `PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test keycloak_e2e`. Never record a Docker test as passed from a run that did not prove that Docker was reachable.
- The workspace sets `warnings = "deny"` (`rs/Cargo.toml` `[workspace.lints.rust]`). A mutation must compile, so it must not leave dead code.
- Do not change any `Cargo.toml` or `rs/Cargo.lock`. Do not touch `ts/`, `py/`, `contracts/`.
- Commits: conventional commits with an allowed scope (`ts/packages/commitlint-config/index.cjs:42`). Rust changes use `feat(rs)` or `test(rs)`. Chart changes use `feat(repo)` (as SMA-703 did). The runbook uses `docs(repo)`. `rs/` docs use `docs(rs)`. Header at most 100 characters, body lines at most 100 characters. No body line may start with a `word: value` shape or hold `#NNN`.
- End every commit message with a blank line and then `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Stage exact paths only (`git add <path> …`). Never `git add -A`, never `--no-verify`, never `git commit --amend`, never `git reset`, never `git stash`, never `git checkout -- <file>`.
- If `git commit` fails with `failed to fill whole buffer`, 1Password is locked: stop and ask the user to unlock it.
- Text in docs, comments and messages uses ASD-STE100 Simplified Technical English: short sentences, active voice, no idiom.
- Do not install host software. Do not run `brew install`.

## Review Focus

Five inputs or failure modes that the spec implies, that no spec test names, and that would most
likely bite a user. Each line names its pinning test and the task that owns it.

1. **A marker refusal hides a missing-claim refusal.** The rate-limit key is (issuer,
   `NotAnAccessToken`) for steps 6, 6b and 6c (spec D4). After a client sends an ID token, the
   first fail-closed refusal of a real access token in the next 10 seconds writes no line.
   Expected: one line, the first one, and the runbook says so. Pinned by
   `missing_claim_shares_the_rate_limit_of_the_marker_refusals` (Task 3) and the § 6 text (Task 8).
2. **Names that differ only in case across the two lists** (`idTokenMarkerClaims: ["jti"]`,
   `accessTokenRequiredClaims: ["JTI"]`). Expected: not an overlap; IAM boots and the chart
   renders, because names compare exactly. Pinned by
   `validate_accepts_required_claims_and_compares_names_exactly` (Task 2) and the `refusals.sh`
   row `required JTI and marker jti` (Task 7).
3. **A repeated top-level member that IAM does not read** (`"foo":1,"foo":2`). It passes today.
   Expected: once only the required list is set, the strict decode refuses it as `Malformed`
   (spec D3, § 7). Pinned by `unread_duplicate_member_is_malformed_once_a_required_claim_is_set`
   (Task 3) and a CHANGELOG line (Task 8).
4. **`--reuse-values` from a release before SMA-703, with `--set oidc.idTokenMarkerClaims=null`
   and the required list set.** Helm keeps a nil marker value. Expected: the overlap check reads
   nil as `[]`, and the render succeeds with the required suffix. Pinned by the `env.sh` rows
   `R7 reuse-values-nil-guard` and `R8 nil-markers-with-required` on a chart copy without both
   keys (Task 7).
5. **The wire answer of a `MissingClaim` refusal.** Expected: the same `401` with
   `invalid-token` as every other token defect; the defect is not exposed. Pinned by the `send`
   assertions in the T19 block of `zitadel_e2e.rs` (Task 4).

## Facts confirmed during planning (2026-10-06)

- A scratch copy of `rs/` with the Task 2 and Task 3 code below compiled; all 974 `paigasus-iam`
  lib tests and all 136 `paigasus-iam-core` tests passed; `cargo fmt --check` and
  `cargo clippy -p paigasus-iam -p paigasus-iam-core --all-targets --locked -- -D warnings` were
  clean. The Rust code in this plan is the `rustfmt` output.
- Before the Task 3 implementation, exactly 12 of the new validator tests failed (listed in Task 3
  Step 2), and the config tests passed.
- The three mutations of Task 6 each compiled and reddened exactly the tests that Task 6 lists.
  Mutation M1 also reddened `zitadel_e2e` at the T19 `expect_err("an ID token must not
  authenticate")`.
- `PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test zitadel_e2e` passed
  in 8 s with the Task 4 code. `keycloak_e2e` passed in 10 s.
- MEASURED (Keycloak 26.4, the Task 1 print): `jti` is `true` on all five Keycloak tokens: the
  password-grant ID token and access token, the refresh-grant ID token and access token, and the
  DPoP-bound access token. So the split does NOT hold for Keycloak. Task 1 still runs the
  measurement (the spec requires it), and it gives the code for both results.
- A scratch copy of the chart with the Task 7 code passed `env.sh`, `refusals.sh`, `render.sh`,
  `maps.sh`, `ingress.sh`, `names.sh` and `ca-bundle.sh` under `/bin/bash` 3.2, and the golden
  files stayed byte-identical. Before the template change, the new rows failed as Task 7 Step 3
  lists.
- MEASURED: the `env.sh` M8 setup check `grep -q 'idTokenMarkerClaims'` matches ANY line. The new
  `values.yaml` comment names `oidc.idTokenMarkerClaims` (spec D5), so M8 reported
  `FAIL [M8 setup]: the chart copy still has idTokenMarkerClaims in values.yaml`. Task 7 anchors
  that grep to the key line `^  idTokenMarkerClaims:`.

---

### Task 1: Measure `jti` on the Keycloak tokens (spec D8, T20 part 1)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs` (module doc `:14-15`, a refresh grant after the `cnf` assertion `:173-174`, assertions after the `cnf.jkt` assertion `:201`, a `has_claim` helper before `jwt_payload` `:364`)
- Modify: `docs/superpowers/specs/2026-10-06-sma-731-iam-required-access-token-claims-design.md:54-55` (§ 3, new F4)

**Interfaces:**
- Consumes: nothing new.
- Produces: in `keycloak_e2e.rs`, the locals `refreshed_access: String` and `refreshed_id: String`, and `fn has_claim(claims: &Value, name: &str) -> bool`. The spec fact F4. The decision for Task 5 and Task 8: "split" (branch A) or "no split" (branch B).

- [ ] **Step 1: Add the refresh grant and the `has_claim` helper**

In `rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs`, Edit `old_string`:

```rust
    // SMA-690 AC 2: Keycloak's plain Bearer access token has no `cnf`.
    assert!(access_claims.get("cnf").is_none(), "keycloak Bearer access token must carry no cnf: {access_claims}");
```

`new_string`:

```rust
    // SMA-690 AC 2: Keycloak's plain Bearer access token has no `cnf`.
    assert!(access_claims.get("cnf").is_none(), "keycloak Bearer access token must carry no cnf: {access_claims}");

    // SMA-731 D8: one refresh grant with the refresh token of the password grant. Keycloak returns
    // a new ID token and a new access token. The failure message prints the body: an OAuth error
    // body has no token.
    let refresh_token = token_body["refresh_token"].as_str().expect("refresh_token in token response").to_string();
    let refresh_response = http
        .post(&token_url)
        .form(&[("grant_type", "refresh_token"), ("client_id", "paigasus-cli"), ("refresh_token", refresh_token.as_str())])
        .send()
        .await
        .expect("refresh request");
    let refresh_status = refresh_response.status();
    let refresh_body: Value = refresh_response.json().await.expect("refresh response json");
    assert!(refresh_status.is_success(), "refresh grant failed ({refresh_status}): {refresh_body}\n{}", dump_logs(&keycloak).await);
    let refreshed_access = refresh_body["access_token"].as_str().expect("access_token in refresh response").to_string();
    let refreshed_id = refresh_body["id_token"].as_str().expect("id_token in refresh response (scope=openid)").to_string();
```

Edit `old_string`:

```rust
/// Decodes a JWT's payload segment WITHOUT verifying it — test inspection only.
```

`new_string`:

```rust
/// True when the payload has a top-level member `name` whose value is not JSON `null` (the
/// presence rule of SMA-731 D3).
fn has_claim(claims: &Value, name: &str) -> bool {
    claims.get(name).is_some_and(|value| !value.is_null())
}

/// Decodes a JWT's payload segment WITHOUT verifying it — test inspection only.
```

- [ ] **Step 2: Add the TEMPORARY measurement print**

Edit `old_string`:

```rust
    assert_eq!(dpop_claims["cnf"]["jkt"], jwk_thumbprint(&dpop_x, &dpop_y), "cnf.jkt must be the RFC 7638 thumbprint of the proof key");
```

`new_string`:

```rust
    assert_eq!(dpop_claims["cnf"]["jkt"], jwk_thumbprint(&dpop_x, &dpop_y), "cnf.jkt must be the RFC 7638 thumbprint of the proof key");
    // SMA-731 D8 TEMPORARY measurement: delete these lines before the commit.
    for (label, token) in [
        ("password ID token", &id_token),
        ("password access token", &access_token),
        ("refreshed ID token", &refreshed_id),
        ("refreshed access token", &refreshed_access),
        ("DPoP access token", &dpop_token),
    ] {
        eprintln!("SMA-731 jti: {label}: {}", has_claim(&jwt_payload(token), "jti"));
    }
```

The print shows booleans only, never token material.

- [ ] **Step 3: Run the Keycloak suite and read the five values**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims/rs
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test keycloak_e2e --no-capture 2>&1 | grep -E 'SMA-731 jti|PASS|FAIL'
```

Expected (MEASURED during planning, Keycloak 26.4): five lines, all `true`, and the test PASSES:

```text
SMA-731 jti: password ID token: true
SMA-731 jti: password access token: true
SMA-731 jti: refreshed ID token: true
SMA-731 jti: refreshed access token: true
SMA-731 jti: DPoP access token: true
        PASS [  10.128s] (1/1) paigasus-iam::keycloak_e2e keycloak_end_to_end_config_only_oidc
```

Decide the branch from YOUR output:
- **Branch A ("split")**: both ID-token lines are `false` AND all three access-token lines are
  `true`.
- **Branch B ("no split")**: any other result. The planning measurement is branch B.

If the refresh request fails with `id_token in refresh response`, Keycloak returned no ID token on
the refresh grant. Stop and report this to the coordinator: the spec D8 assumes one.

- [ ] **Step 4: Replace the print with the final assertions**

Branch B (the planning result). Edit `old_string` (the print block of Step 2):

```rust
    assert_eq!(dpop_claims["cnf"]["jkt"], jwk_thumbprint(&dpop_x, &dpop_y), "cnf.jkt must be the RFC 7638 thumbprint of the proof key");
    // SMA-731 D8 TEMPORARY measurement: delete these lines before the commit.
    for (label, token) in [
        ("password ID token", &id_token),
        ("password access token", &access_token),
        ("refreshed ID token", &refreshed_id),
        ("refreshed access token", &refreshed_access),
        ("DPoP access token", &dpop_token),
    ] {
        eprintln!("SMA-731 jti: {label}: {}", has_claim(&jwt_payload(token), "jti"));
    }
```

`new_string`:

```rust
    assert_eq!(dpop_claims["cnf"]["jkt"], jwk_thumbprint(&dpop_x, &dpop_y), "cnf.jkt must be the RFC 7638 thumbprint of the proof key");
    // SMA-731 F4 (T20): measured on Keycloak 26.4, 2026-10-06. `jti` does not separate the
    // Keycloak ID token from the access token, so `access_token_required_claims` does not help
    // for Keycloak, and the `typ` check (SMA-686) stays the Keycloak defence. The values below are
    // the measured ones. A Keycloak image bump that changes one fails here.
    for (label, token, carries_jti) in [
        ("password ID token", &id_token, true),
        ("password access token", &access_token, true),
        ("refreshed ID token", &refreshed_id, true),
        ("refreshed access token", &refreshed_access, true),
        ("DPoP access token", &dpop_token, true),
    ] {
        assert_eq!(has_claim(&jwt_payload(token), "jti"), carries_jti, "{label}: the jti presence changed (SMA-731 F4)");
    }
```

If your Step 3 output differs from the planning result but is still branch B, change each `true`
in the five tuples to the value that Step 3 printed for that label.

Branch A (only if Step 3 showed the split). Use this `new_string` instead:

```rust
    assert_eq!(dpop_claims["cnf"]["jkt"], jwk_thumbprint(&dpop_x, &dpop_y), "cnf.jkt must be the RFC 7638 thumbprint of the proof key");
    // SMA-731 F4 (T20): measured on Keycloak 26.4, 2026-10-06. `jti` is on every access token
    // (password grant, refresh grant, DPoP-bound) and on no ID token (password grant, refresh
    // grant). A Keycloak image bump that changes this fails here.
    for (label, token) in [("password ID token", &id_token), ("refreshed ID token", &refreshed_id)] {
        assert!(!has_claim(&jwt_payload(token), "jti"), "{label} must NOT carry jti (SMA-731 F4)");
    }
    for (label, token) in [
        ("password access token", &access_token),
        ("refreshed access token", &refreshed_access),
        ("DPoP access token", &dpop_token),
    ] {
        assert!(has_claim(&jwt_payload(token), "jti"), "{label} must carry jti (SMA-731 F4)");
    }
```

Then add the module doc paragraph. Edit `old_string`:

```rust
//! SMA-690: the test also gets a DPoP-bound token from the same client and asserts that IAM
//! refuses it as SenderConstrained.
```

`new_string`:

```rust
//! SMA-690: the test also gets a DPoP-bound token from the same client and asserts that IAM
//! refuses it as SenderConstrained.
//!
//! SMA-731 (spec D8, T20): the test also runs one refresh grant, and pins whether `jti` is on the
//! ID token and the access token of the password grant, of the refresh grant, and on the
//! DPoP-bound access token.
```

Check that the print is gone:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims
grep -c 'eprintln!("SMA-731' rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs
```

Expected: `0`.

- [ ] **Step 5: Record F4 in the spec**

In `docs/superpowers/specs/2026-10-06-sma-731-iam-required-access-token-claims-design.md`, Edit
`old_string`:

```markdown
- F3. Not measured: whether other IdPs follow this split. Keycloak 26.4 is measured by this issue
  (D8, T20). The result goes into this section as F4 when the plan's first task has run.
```

`new_string` for branch B:

```markdown
- F3. Not measured: whether other IdPs follow this split. Keycloak 26.4 is measured by this issue
  (D8, T20). The result goes into this section as F4 when the plan's first task has run.
- F4. Keycloak 26.4 (measured 2026-10-06 by `tests/keycloak_e2e.rs`): `jti` is on the ID token
  and the access token of the password grant, on the ID token and the access token of the
  refresh grant, and on the DPoP-bound access token. So `jti` does not separate the Keycloak
  tokens. The runbook gives no Keycloak recipe, and the `typ` check (SMA-686) stays the Keycloak
  defence (D6, D8). T20 asserts these five values.
```

`new_string` for branch A:

```markdown
- F3. Not measured: whether other IdPs follow this split. Keycloak 26.4 is measured by this issue
  (D8, T20). The result goes into this section as F4 when the plan's first task has run.
- F4. Keycloak 26.4 (measured 2026-10-06 by `tests/keycloak_e2e.rs`): `jti` is on the access
  token of the password grant, of the refresh grant and on the DPoP-bound access token, and on
  no ID token (password grant, refresh grant). The runbook gives `["jti"]` for Keycloak as a
  second defence next to the `typ` check (D6). T20 asserts these values.
```

- [ ] **Step 6: Run the suite again, fmt and clippy**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims/rs
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test keycloak_e2e
cargo fmt --check -p paigasus-iam
cargo clippy -p paigasus-iam --all-targets --locked -- -D warnings
```

Expected: `keycloak_end_to_end_config_only_oidc` PASSES; fmt prints nothing; clippy exits 0.

- [ ] **Step 7: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims
git add rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs \
  docs/superpowers/specs/2026-10-06-sma-731-iam-required-access-token-claims-design.md
git commit -m "test(rs): pin jti on the Keycloak ID and access tokens (SMA-731)" \
  -m "keycloak_e2e runs one refresh grant and asserts whether jti is on the ID token and the
access token of the password grant and of the refresh grant, and on the DPoP-bound access
token. The spec records the measured result as F4." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git log --oneline -1
```

---

### Task 2: The `access_token_required_claims` config field and its boot rules

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/config.rs` (field `:185-190`, constant `:267-271`, `validate` doc `:1117-1120`, `validate` loop `:1143-1160`, new tests before `bootstrap_admins_env_in_the_chart_form_parses` `:1730`)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs:641-647` (test helper `issuer_config`)
- Modify: `rs/crates/services/paigasus-iam/tests/support/mod.rs:505`
- Modify: `rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs` (the `IssuerConfig` literal in `keycloak_config`)
- Modify: `rs/crates/services/paigasus-iam/tests/zitadel_e2e.rs` (the `IssuerConfig` literal in `zitadel_config`)
- Modify: `rs/crates/services/paigasus-iam/tests/authn_private_ca.rs:39`

**Interfaces:**
- Consumes: nothing new.
- Produces: `pub struct IssuerConfig { …, pub access_token_required_claims: Vec<String> }` with `#[serde(default)]`; `const RESERVED_CLAIM_NAMES: [&str; 4]` and `fn validate_claim_names(issuer: &str, field: &str, names: &[String], reserved_reason: &str) -> Result<(), String>` (both private to `config.rs`); new `IamConfig::validate` errors that start with `authn.issuers[<issuer>].access_token_required_claims`.

- [ ] **Step 1: Write the failing config tests (T15, T17, Review Focus 2)**

In `rs/crates/services/paigasus-iam/src/config.rs` (`mod tests`), Edit `old_string`:

```rust
    #[test]
    fn bootstrap_admins_env_in_the_chart_form_parses() {
```

`new_string`:

```rust
    #[test]
    fn issuers_env_in_the_chart_form_parses_access_token_required_claims() {
        // SMA-731 T17: the exact strings that charts/paigasus renders into IAM_AUTHN__ISSUERS
        // (env.sh rows R3 and R4), and the same string without the key (row R1).
        let issuer = "https://idp.example.test/realms/paigasus";
        let cases: [(String, Vec<&str>, Vec<&str>); 3] = [
            (
                format!(r#"[{{issuer="{issuer}",audiences=["paigasus-console"],access_token_required_claims=["jti"]}}]"#),
                vec![],
                vec!["jti"],
            ),
            (
                format!(r#"[{{issuer="{issuer}",audiences=["paigasus-console"],id_token_marker_claims=["at_hash","azp"],access_token_required_claims=["jti"]}}]"#),
                vec!["at_hash", "azp"],
                vec!["jti"],
            ),
            (format!(r#"[{{issuer="{issuer}",audiences=["paigasus-console"]}}]"#), vec![], vec![]),
        ];
        for (issuers, markers, required) in cases {
            figment::Jail::expect_with(|jail| {
                jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
                jail.set_env("IAM_API_KEYS__PEPPER", valid_pepper_b64());
                jail.set_env("IAM_AUTHN__ISSUERS", &issuers);
                let cfg: IamConfig = IamConfig::figment().extract()?;
                assert_eq!(cfg.authn.issuers.len(), 1);
                assert_eq!(cfg.authn.issuers[0].id_token_marker_claims, markers, "{issuers}");
                assert_eq!(cfg.authn.issuers[0].access_token_required_claims, required, "{issuers}");
                assert!(cfg.validate().is_ok(), "the chart's issuer string must pass validation: {issuers}");
                Ok(())
            });
        }
    }

    #[test]
    fn a_misspelled_required_claims_key_is_ignored() {
        // SMA-731 § 6: IssuerConfig ignores an unknown key, so a typo gives an empty list and no
        // error. The missing boot line is the only sign (the runbook says so).
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.set_env("IAM_API_KEYS__PEPPER", valid_pepper_b64());
            jail.set_env(
                "IAM_AUTHN__ISSUERS",
                r#"[{issuer="https://idp.example.test/realms/paigasus",audiences=["paigasus-console"],access_token_required_claim=["jti"]}]"#,
            );
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.authn.issuers[0].access_token_required_claims.is_empty());
            assert!(cfg.validate().is_ok());
            Ok(())
        });
    }

    #[test]
    fn validate_refuses_bad_access_token_required_claims() {
        // SMA-731 T15: each D2 rule fails validate, and the message names the issuer and the field.
        let cases: [(&[&str], &str); 9] = [
            (&[""], "contains an empty name"),
            (&[" jti"], "leading or trailing whitespace"),
            (&["jti\n"], "leading or trailing whitespace"),
            (&[" "], "leading or trailing whitespace"),
            (&["iss"], "must not contain \"iss\": every token that IAM accepts carries it, so the name has no effect"),
            (&["sub"], "must not contain \"sub\""),
            (&["aud"], "must not contain \"aud\""),
            (&["exp"], "must not contain \"exp\""),
            (&["jti", "nbf", "jti"], "contains the name \"jti\" twice"),
        ];
        for (names, want) in cases {
            let mut cfg = load_minimal_config();
            cfg.authn.issuers[0].access_token_required_claims = names.iter().map(|name| (*name).to_string()).collect();
            let err = cfg.validate().expect_err("a bad required list must fail validation");
            assert!(
                err.contains("authn.issuers[https://idp.example.com/realms/acme].access_token_required_claims"),
                "{names:?}: the message names the issuer and the key: {err}"
            );
            assert!(err.contains(want), "{names:?}: want {want:?} in {err}");
        }
    }

    #[test]
    fn validate_refuses_a_name_in_both_claim_lists() {
        // SMA-731 T15 (D2): IAM would refuse every token of the issuer. The message names both
        // fields and the name.
        let mut cfg = load_minimal_config();
        cfg.authn.issuers[0].id_token_marker_claims = vec!["at_hash".to_string(), "azp".to_string()];
        cfg.authn.issuers[0].access_token_required_claims = vec!["jti".to_string(), "azp".to_string()];
        let err = cfg.validate().expect_err("an overlap must fail validation");
        for want in [
            "authn.issuers[https://idp.example.com/realms/acme].access_token_required_claims",
            "authn.issuers[https://idp.example.com/realms/acme].id_token_marker_claims",
            "\"azp\"",
            "IAM would refuse every token of this issuer",
        ] {
            assert!(err.contains(want), "want {want:?} in {err}");
        }
    }

    #[test]
    fn validate_accepts_required_claims_and_compares_names_exactly() {
        // SMA-731 D2 / Review Focus 2: the Zitadel recipe passes alone and with the markers; names
        // that differ only in case are two names, also across the two lists.
        let cases: [(&[&str], &[&str]); 5] = [(&[], &["jti"]), (&["at_hash", "azp"], &["jti"]), (&["jti"], &["JTI"]), (&[], &["ISS", "jti", "JTI"]), (&[], &[])];
        for (markers, required) in cases {
            let mut cfg = load_minimal_config();
            cfg.authn.issuers[0].id_token_marker_claims = markers.iter().map(|name| (*name).to_string()).collect();
            cfg.authn.issuers[0].access_token_required_claims = required.iter().map(|name| (*name).to_string()).collect();
            assert!(cfg.validate().is_ok(), "{markers:?} / {required:?} must pass validation: {:?}", cfg.validate());
        }
    }

    #[test]
    fn bootstrap_admins_env_in_the_chart_form_parses() {
```

T16 is the existing test `validate_refuses_bad_id_token_marker_claims`. Do not change it.

- [ ] **Step 2: Run the tests and see them fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims/rs
cargo nextest run -p paigasus-iam --lib --locked -E 'test(/config::tests/)'
```

Expected: the build fails with ``error[E0609]: no field `access_token_required_claims` on type `IssuerConfig` `` (one for each use). No test runs.

- [ ] **Step 3: Add the field**

Edit `old_string`:

```rust
    #[serde(default)]
    pub id_token_marker_claims: Vec<String>,
}
```

`new_string`:

```rust
    #[serde(default)]
    pub id_token_marker_claims: Vec<String>,
    /// Claim names that every accepted token of this issuer must carry (SMA-731). Empty by default.
    /// A verified token that does not carry one of them, or carries it with the value JSON `null`,
    /// is refused as `NotAnAccessToken`. For Zitadel, use `["jti"]` (SMA-731 spec § 3, F1).
    #[serde(default)]
    pub access_token_required_claims: Vec<String>,
}
```

- [ ] **Step 4: Rename the constant and add the shared function**

Edit `old_string`:

```rust
/// Claim names that every token IAM accepts carries (SMA-703 D2). A marker list that names one
/// would make IAM refuse every token of that issuer, so `IamConfig::validate` refuses it.
/// Keep this list equal to the reserved names in `paigasus.validateIdTokenMarkerClaims` in
/// `charts/paigasus/templates/_iam-backend.tpl`.
const RESERVED_MARKER_CLAIMS: [&str; 4] = ["iss", "sub", "aud", "exp"];
```

`new_string`:

```rust
/// Claim names that every token IAM accepts carries (SMA-703 D2, SMA-731 D2). In
/// `id_token_marker_claims` such a name would make IAM refuse every token of the issuer. In
/// `access_token_required_claims` it would have no effect. So `IamConfig::validate` refuses it in
/// both lists. Keep this list equal to the reserved names in `paigasus.validateClaimNameList` in
/// `charts/paigasus/templates/_iam-backend.tpl`.
const RESERVED_CLAIM_NAMES: [&str; 4] = ["iss", "sub", "aud", "exp"];

/// The boot rules for one claim-name list of an issuer (SMA-703 D2, SMA-731 D2): no empty name, no
/// leading or trailing whitespace, no name of `RESERVED_CLAIM_NAMES`, and no name twice. `field`
/// is the `IssuerConfig` field name, and `reserved_reason` ends the reserved-name message. Both
/// lists use this one function, so their rules cannot drift apart. Names compare exactly: JSON
/// member names are case-sensitive.
fn validate_claim_names(issuer: &str, field: &str, names: &[String], reserved_reason: &str) -> Result<(), String> {
    let mut seen = HashSet::with_capacity(names.len());
    for name in names {
        if name.is_empty() {
            return Err(format!("authn.issuers[{issuer}].{field} contains an empty name"));
        }
        if name.trim() != name {
            return Err(format!("authn.issuers[{issuer}].{field} has a name with leading or trailing whitespace: {name:?}"));
        }
        if RESERVED_CLAIM_NAMES.contains(&name.as_str()) {
            return Err(format!("authn.issuers[{issuer}].{field} must not contain {name:?}: {reserved_reason}"));
        }
        if !seen.insert(name.as_str()) {
            return Err(format!("authn.issuers[{issuer}].{field} contains the name {name:?} twice"));
        }
    }
    Ok(())
}
```

- [ ] **Step 5: Use the shared function and the overlap rule in `validate`**

Edit `old_string`:

```rust
            // SMA-703 D2. Names compare exactly: JSON member names are case-sensitive.
            let mut seen_markers = HashSet::with_capacity(issuer_cfg.id_token_marker_claims.len());
            for name in &issuer_cfg.id_token_marker_claims {
                if name.is_empty() {
                    return Err(format!("authn.issuers[{trimmed}].id_token_marker_claims contains an empty name"));
                }
                if name.trim() != name {
                    return Err(format!("authn.issuers[{trimmed}].id_token_marker_claims has a name with leading or trailing whitespace: {name:?}"));
                }
                if RESERVED_MARKER_CLAIMS.contains(&name.as_str()) {
                    return Err(format!(
                        "authn.issuers[{trimmed}].id_token_marker_claims must not contain {name:?}: every token that IAM accepts carries it, so IAM would refuse every token of this issuer"
                    ));
                }
                if !seen_markers.insert(name.as_str()) {
                    return Err(format!("authn.issuers[{trimmed}].id_token_marker_claims contains the name {name:?} twice"));
                }
            }
        }
```

`new_string`:

```rust
            // SMA-703 D2 and SMA-731 D2: one shared function for both lists.
            validate_claim_names(
                trimmed,
                "id_token_marker_claims",
                &issuer_cfg.id_token_marker_claims,
                "every token that IAM accepts carries it, so IAM would refuse every token of this issuer",
            )?;
            validate_claim_names(
                trimmed,
                "access_token_required_claims",
                &issuer_cfg.access_token_required_claims,
                "every token that IAM accepts carries it, so the name has no effect",
            )?;
            // SMA-731 D2: a name in both lists makes IAM refuse every token of the issuer. A token
            // with the claim is an ID token by the marker rule, and a token without it misses it.
            if let Some(name) = issuer_cfg.access_token_required_claims.iter().find(|name| issuer_cfg.id_token_marker_claims.contains(name)) {
                return Err(format!(
                    "authn.issuers[{trimmed}].access_token_required_claims and authn.issuers[{trimmed}].id_token_marker_claims both contain {name:?}: IAM would refuse every token of this issuer"
                ));
            }
        }
```

Then extend the `validate` doc comment. Edit `old_string`:

```rust
    /// `iss`/`sub`/`aud`/`exp`, and occurs once in its list. Also (SMA-700): the `[authn.dpop]`
```

`new_string`:

```rust
    /// `iss`/`sub`/`aud`/`exp`, and occurs once in its list. Also (SMA-731 D2): the same rules
    /// for each `access_token_required_claims` name, and no name in both lists of one issuer.
    /// Also (SMA-700): the `[authn.dpop]`
```

- [ ] **Step 6: Update the five `IssuerConfig` literals**

`rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs` (test helper `issuer_config`), Edit `old_string`:

```rust
            jit_provisioning: true,
            id_token_marker_claims: Vec::new(),
        }
    }
```

`new_string`:

```rust
            jit_provisioning: true,
            id_token_marker_claims: Vec::new(),
            access_token_required_claims: Vec::new(),
        }
    }
```

`rs/crates/services/paigasus-iam/tests/support/mod.rs`, Edit `old_string`:

```rust
                    id_token_marker_claims: Vec::new(),
```

`new_string`:

```rust
                    id_token_marker_claims: Vec::new(),
                    access_token_required_claims: Vec::new(),
```

`rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs`, Edit `old_string`:

```rust
                id_token_marker_claims: Vec::new(),
```

`new_string`:

```rust
                id_token_marker_claims: Vec::new(),
                access_token_required_claims: Vec::new(),
```

`rs/crates/services/paigasus-iam/tests/zitadel_e2e.rs`, Edit `old_string`:

```rust
                id_token_marker_claims: markers.iter().map(|name| (*name).to_string()).collect(),
```

`new_string`:

```rust
                id_token_marker_claims: markers.iter().map(|name| (*name).to_string()).collect(),
                access_token_required_claims: Vec::new(),
```

`rs/crates/services/paigasus-iam/tests/authn_private_ca.rs`, Edit `old_string`:

```rust
            id_token_marker_claims: Vec::new(),
```

`new_string`:

```rust
            id_token_marker_claims: Vec::new(),
            access_token_required_claims: Vec::new(),
```

- [ ] **Step 7: Run the tests and see them pass**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims/rs
cargo nextest run -p paigasus-iam --lib --locked -E 'test(/config::tests/)'
```

Expected: PASS, the new `issuers_env_in_the_chart_form_parses_access_token_required_claims`,
`a_misspelled_required_claims_key_is_ignored`, `validate_refuses_bad_access_token_required_claims`,
`validate_refuses_a_name_in_both_claim_lists` and
`validate_accepts_required_claims_and_compares_names_exactly` included, and the unchanged SMA-703
tests `validate_refuses_bad_id_token_marker_claims` (T16) and
`issuers_env_in_the_chart_form_parses_id_token_marker_claims`.

- [ ] **Step 8: Run the whole lib suite, fmt and clippy**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims/rs
cargo nextest run -p paigasus-iam --lib --locked
cargo fmt --check -p paigasus-iam
cargo clippy -p paigasus-iam --all-targets --locked -- -D warnings
```

Expected: all lib tests PASS; fmt prints nothing; clippy exits 0 (it compiles the four
integration-test files with the new literals).

- [ ] **Step 9: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims
git add rs/crates/services/paigasus-iam/src/config.rs \
  rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs \
  rs/crates/services/paigasus-iam/tests/support/mod.rs \
  rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs \
  rs/crates/services/paigasus-iam/tests/zitadel_e2e.rs \
  rs/crates/services/paigasus-iam/tests/authn_private_ca.rs
git commit -m "feat(rs): add per-issuer access_token_required_claims to the IAM config (SMA-731)" \
  -m "IssuerConfig gets an optional list of claim names, empty by default. One shared function
now validates both claim lists, so the SMA-703 rules and messages apply to the new list too.
IamConfig::validate also refuses a name that is in both lists of one issuer. The figment tests
parse the exact forms that the chart renders." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git log --oneline -1
```

---

### Task 3: The validator refuses a token that does not carry a required claim

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs` (module doc `:10-16`, `RefusalDetail` `:58-73`, `ConfiguredIssuer` `:78-84`, `new` `:107-131`, `log_refusal` doc and arms `:144-189`, the message constant `:192-194`, after `configured_marker` `:309-311`, `authenticate` decode and step 6b `:457-478`, new tests at the end of `mod tests`)
- Modify: `rs/crates/libs/paigasus-iam-core/src/authn.rs:209-211` (`TokenDefect::NotAnAccessToken` doc)

**Interfaces:**
- Consumes: `IssuerConfig::access_token_required_claims: Vec<String>` (Task 2); the existing test helpers `es256_keypair`, `sign`, `sign_raw_payload`, `make_authenticator`, `issuer_config`, `issuer_with_markers`, `merged`, `without`, `assert_not_an_access_token`, `StubFetcher`, `capture_logs`, the fixtures `zitadel_m1_*`, `zitadel_m4_*`, `zitadel_m5b_*`, and the constants `ISSUER`, `SECOND_ISSUER`, `JKT`, `ZITADEL_*`, `NOT_ACCESS_TOKEN_REFUSAL`, `MARKER_BOOT_LINE`, `BINDING_REFUSAL`.
- Produces (all private to `validator.rs`): `#[derive(Debug, PartialEq, Eq)] enum RefusalDetail<'a>` with the new variant `MissingClaim(&'a str)`; `struct ClaimRules { id_token_markers: Vec<String>, required: Vec<String> }` with `fn needs_strict_decode(&self) -> bool` and `fn refusal(&self, members: &serde_json::Map<String, serde_json::Value>) -> Option<RefusalDetail<'_>>`; `ConfiguredIssuer::claim_rules: ClaimRules` (replaces `id_token_marker_claims`); `fn missing_required_claim<'a>(members: &serde_json::Map<String, serde_json::Value>, names: &'a [String]) -> Option<&'a str>`; `const MISSING_CLAIM_MESSAGE: &str`; test helpers `issuer_with_rules`, `authenticate_rules`, `authenticate_rules_as`, `authenticate_raw_rules`, `claim_rules`, `members`.

- [ ] **Step 1: Write the failing validator tests (T1-T14a, Review Focus 1 and 3)**

In `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs`, Edit `old_string` (the end
of the file):

```rust
        let _without = make_authenticator(StubFetcher::new(jwk), vec![issuer_config(ISSUER, &["aud"])], 60, 16_384);
        assert!(!logs.text().contains(MARKER_BOOT_LINE), "no boot line for an empty list:\n{}", logs.text());
    }
}
```

`new_string`:

```rust
        let _without = make_authenticator(StubFetcher::new(jwk), vec![issuer_config(ISSUER, &["aud"])], 60, 16_384);
        assert!(!logs.text().contains(MARKER_BOOT_LINE), "no boot line for an empty list:\n{}", logs.text());
    }

    // ---- SMA-731: configured required claims ------------------------------------------------
    //
    // The fixtures are the SMA-703 Zitadel fixtures above (M1, M4a, M5b). Every Zitadel access
    // token has `jti`, and no Zitadel ID token has it (SMA-731 spec § 3, F1).

    /// The runbook recipe for Zitadel (SMA-731 spec D6).
    const REQUIRED_JTI: [&str; 1] = ["jti"];
    /// The SMA-731 refusal message (spec D4). It differs from `NOT_ACCESS_TOKEN_REFUSAL`.
    const MISSING_CLAIM_REFUSAL: &str = "it does not carry a claim that the issuer configuration requires";
    /// The SMA-731 boot line (spec D2).
    const REQUIRED_BOOT_LINE: &str = "does not carry every configured required claim";

    fn issuer_with_rules(issuer: &str, audiences: &[&str], markers: &[&str], required: &[&str]) -> IssuerConfig {
        IssuerConfig {
            access_token_required_claims: required.iter().map(|name| (*name).to_string()).collect(),
            ..issuer_with_markers(issuer, audiences, markers)
        }
    }

    /// Signs `claims` with a fresh key and authenticates it on `scheme` against `ISSUER`,
    /// configured with the audience `ZITADEL_PROJECT_ID`, the marker claims `markers` and the
    /// required claims `required`.
    async fn authenticate_rules_as(markers: &[&str], required: &[&str], claims: &serde_json::Value, scheme: TokenScheme) -> Result<ValidatedClaims, AuthnError> {
        let (encoding_key, jwk, kid) = es256_keypair();
        let token = sign(&encoding_key, Some(&kid), claims);
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_with_rules(ISSUER, &[ZITADEL_PROJECT_ID], markers, required)], 60, 16_384);
        authenticator.authenticate(&token, scheme).await
    }

    async fn authenticate_rules(markers: &[&str], required: &[&str], claims: &serde_json::Value) -> Result<ValidatedClaims, AuthnError> {
        authenticate_rules_as(markers, required, claims, TokenScheme::Bearer).await
    }

    /// Authenticates a raw payload (the usual test fields, then `members` verbatim) against
    /// `ISSUER` with the audience `aud`, the marker claims `markers` and the required claims
    /// `required`. A raw payload can repeat a member name.
    async fn authenticate_raw_rules(markers: &[&str], required: &[&str], members: &str) -> Result<ValidatedClaims, AuthnError> {
        let exp = Utc::now().timestamp() + 3600;
        let payload = format!(r#"{{"iss":"{ISSUER}","sub":"sub-1","aud":"aud","exp":{exp},"email":"alice@example.com",{members}}}"#);
        let (encoding_key, jwk, kid) = es256_keypair();
        let token = sign_raw_payload(&encoding_key, &kid, &payload);
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_with_rules(ISSUER, &["aud"], markers, required)], 60, 16_384);
        authenticator.authenticate(&token, TokenScheme::Bearer).await
    }

    #[tokio::test]
    async fn required_claim_refuses_zitadel_id_tokens() {
        // SMA-731 T1: with ["jti"] and NO marker claims, each Zitadel ID token is refused.
        for (name, claims) in [("M1 ID token", zitadel_m1_id_token()), ("M4 ID token", zitadel_m4_id_token()), ("M5b ID token", zitadel_m5b_id_token())] {
            assert_not_an_access_token(authenticate_rules(&[], &REQUIRED_JTI, &claims).await, name);
        }
    }

    #[tokio::test]
    async fn required_claim_accepts_zitadel_access_tokens() {
        // SMA-731 T2.
        for (name, claims, subject) in [
            ("M1 access token", zitadel_m1_access_token(), ZITADEL_HUMAN_SUB),
            ("M4 access token", zitadel_m4_access_token(), ZITADEL_HUMAN_SUB),
            ("M5b access token", zitadel_m5b_access_token(), ZITADEL_MACHINE_SUB),
        ] {
            let validated = authenticate_rules(&[], &REQUIRED_JTI, &claims)
                .await
                .unwrap_or_else(|err| panic!("{name}: must be accepted, got {err:?}"));
            assert_eq!(validated.subject, subject, "{name}");
        }
    }

    #[tokio::test]
    async fn null_required_claim_is_missing_and_any_other_value_is_present() {
        // SMA-731 T3 (D3): the SMA-703 null rule with the opposite result.
        let null = merged(zitadel_m1_access_token(), serde_json::json!({ "jti": null }));
        assert_not_an_access_token(authenticate_rules(&[], &REQUIRED_JTI, &null).await, "jti: null");
        for value in [serde_json::json!(""), serde_json::json!(0), serde_json::json!(false), serde_json::json!({}), serde_json::json!([])] {
            let claims = merged(zitadel_m1_access_token(), serde_json::json!({ "jti": value.clone() }));
            authenticate_rules(&[], &REQUIRED_JTI, &claims)
                .await
                .unwrap_or_else(|err| panic!("jti: {value} must count as present, got {err:?}"));
        }
    }

    #[tokio::test]
    async fn the_first_missing_required_claim_is_logged() {
        // SMA-731 T4: with ["jti", "nbf"], a token with `jti` and no `nbf` is refused, and the log
        // names `nbf`.
        let (logs, _guard) = capture_logs();
        let claims = without(zitadel_m1_access_token(), "nbf");
        assert_not_an_access_token(authenticate_rules(&[], &["jti", "nbf"], &claims).await, "no nbf");
        let text = logs.text();
        assert!(text.contains("missing claim nbf"), "the log names the missing claim:\n{text}");
        assert!(!text.contains("missing claim jti"), "jti is present:\n{text}");
    }

    #[tokio::test]
    async fn required_claim_names_are_case_sensitive() {
        // SMA-731 T5 (D2): `JTI` is not `jti`.
        let claims = merged(without(zitadel_m1_access_token(), "jti"), serde_json::json!({ "JTI": "x" }));
        assert_not_an_access_token(authenticate_rules(&[], &REQUIRED_JTI, &claims).await, "JTI only");
    }

    #[tokio::test]
    async fn empty_claim_lists_accept_the_zitadel_id_token() {
        // SMA-731 T6 (G2): with both lists empty, nothing changes. This is the open state.
        authenticate_rules(&[], &[], &zitadel_m1_id_token())
            .await
            .expect("with both lists empty the M1 ID token is accepted (open state)");
    }

    #[tokio::test]
    async fn decode_defects_come_before_the_required_claim_check() {
        // SMA-731 T7 (D3): the decode runs before step 6c.
        let expired = merged(zitadel_m1_id_token(), serde_json::json!({ "exp": Utc::now().timestamp() - 120 }));
        let err = authenticate_rules(&[], &REQUIRED_JTI, &expired).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::Expired)), "got {err:?}");
        let wrong_aud = merged(zitadel_m1_id_token(), serde_json::json!({ "aud": ["other-project"] }));
        let err = authenticate_rules(&[], &REQUIRED_JTI, &wrong_aud).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::AudienceMismatch)), "got {err:?}");
    }

    #[tokio::test]
    async fn keycloak_typ_marker_runs_before_the_required_claims() {
        // SMA-731 T8: step 6 runs first, so a Keycloak `typ: ID` token without `jti` logs `ID`.
        let (logs, _guard) = capture_logs();
        let claims = merged(without(zitadel_m1_access_token(), "jti"), serde_json::json!({ "typ": "ID" }));
        assert_not_an_access_token(authenticate_rules(&[], &REQUIRED_JTI, &claims).await, "typ ID without jti");
        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|line| line.contains(NOT_ACCESS_TOKEN_REFUSAL)).collect();
        assert_eq!(lines.len(), 1, "exactly one refusal line expected, got:\n{text}");
        assert!(lines[0].contains("\"ID\"") || lines[0].contains("=ID"), "the marker is ID: {}", lines[0]);
        assert!(!text.contains("missing claim"), "step 6c must not run:\n{text}");
    }

    #[tokio::test]
    async fn marker_claim_runs_before_the_required_claim() {
        // SMA-731 T9: with both settings, the M1 ID token logs `claim at_hash` (step 6b), not
        // `missing claim jti`, and the M1 access token passes.
        let (logs, _guard) = capture_logs();
        assert_not_an_access_token(authenticate_rules(&ZITADEL_MARKERS, &REQUIRED_JTI, &zitadel_m1_id_token()).await, "M1 ID token");
        let text = logs.text();
        assert!(text.contains("claim at_hash"), "step 6b names the marker claim:\n{text}");
        assert!(!text.contains("missing claim jti"), "step 6c must not run:\n{text}");
        authenticate_rules(&ZITADEL_MARKERS, &REQUIRED_JTI, &zitadel_m1_access_token())
            .await
            .expect("the M1 access token passes the full recipe");
    }

    #[tokio::test]
    async fn required_claim_runs_before_the_sender_constraint_check() {
        // SMA-731 T10: a bound token without `jti` is NotAnAccessToken, not SenderConstrained.
        let (logs, _guard) = capture_logs();
        let claims = merged(without(zitadel_m1_access_token(), "jti"), serde_json::json!({ "cnf": { "jkt": JKT } }));
        assert_not_an_access_token(authenticate_rules(&[], &REQUIRED_JTI, &claims).await, "cnf without jti");
        let text = logs.text();
        assert!(text.contains("missing claim jti"), "step 6c names the claim:\n{text}");
        assert!(!text.contains(BINDING_REFUSAL), "the binding refusal must not run:\n{text}");
    }

    #[tokio::test]
    async fn duplicate_required_claim_member_is_malformed() {
        // SMA-731 T11 (D3): the required list alone moves the issuer onto the strict decode, so a
        // `null` copy cannot hide or fake the claim. Both orders are Malformed.
        for members in [r#""jti":null,"jti":"x""#, r#""jti":"x","jti":null"#] {
            let err = authenticate_raw_rules(&[], &REQUIRED_JTI, members).await.unwrap_err();
            assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::Malformed)), "{members}: got {err:?}");
        }
    }

    #[tokio::test]
    async fn unread_duplicate_member_is_malformed_once_a_required_claim_is_set() {
        // Review Focus 3: a repeated member that IAM does not read passes the plain decode. With
        // only the required list set, the strict decode refuses it as Malformed.
        authenticate_raw_rules(&[], &[], r#""jti":"x","foo":1,"foo":2"#)
            .await
            .expect("the plain path ignores a repeated unread member");
        let err = authenticate_raw_rules(&[], &REQUIRED_JTI, r#""jti":"x","foo":1,"foo":2"#).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::Malformed)), "got {err:?}");
    }

    #[tokio::test]
    async fn required_claims_are_per_issuer() {
        // SMA-731 T12: one issuer with ["jti"], one with an empty list, one key for both.
        let (encoding_key, jwk, kid) = es256_keypair();
        let authenticator = make_authenticator(
            StubFetcher::new(jwk),
            vec![
                issuer_with_rules(ISSUER, &[ZITADEL_PROJECT_ID], &[], &REQUIRED_JTI),
                issuer_config(SECOND_ISSUER, &[ZITADEL_PROJECT_ID]),
            ],
            60,
            16_384,
        );
        let first = sign(&encoding_key, Some(&kid), &zitadel_m1_id_token());
        assert_not_an_access_token(authenticator.authenticate(&first, TokenScheme::Bearer).await, "ID token of the first issuer");
        let second = sign(&encoding_key, Some(&kid), &merged(zitadel_m1_id_token(), serde_json::json!({ "iss": SECOND_ISSUER })));
        let validated = authenticator.authenticate(&second, TokenScheme::Bearer).await.expect("the second issuer has no required claims");
        assert_eq!(validated.issuer.as_str(), SECOND_ISSUER);
    }

    #[tokio::test]
    async fn missing_claim_refusal_logs_its_own_message_issuer_and_name_only() {
        // SMA-731 T13 (D4): its own message, the issuer and `missing claim jti`; no claim value,
        // subject or email; one line for three refusals.
        let (logs, _guard) = capture_logs();
        let (encoding_key, jwk, kid) = es256_keypair();
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_with_rules(ISSUER, &[ZITADEL_PROJECT_ID], &[], &REQUIRED_JTI)], 60, 16_384);
        let claims = merged(zitadel_m1_id_token(), serde_json::json!({ "email": "alice@example.com" }));
        for _ in 0..3 {
            let token = sign(&encoding_key, Some(&kid), &claims);
            assert_not_an_access_token(authenticator.authenticate(&token, TokenScheme::Bearer).await, "M1 ID token");
        }
        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|line| line.contains(MISSING_CLAIM_REFUSAL)).collect();
        assert_eq!(lines.len(), 1, "exactly one refusal line expected, got:\n{text}");
        let line = lines[0];
        assert!(line.contains("INFO"), "the refusal logs at info: {line}");
        assert!(line.contains(ISSUER), "the refusal names the issuer: {line}");
        assert!(line.contains("missing claim jti"), "the refusal names the missing claim: {line}");
        assert!(!text.contains(NOT_ACCESS_TOKEN_REFUSAL), "the MissingClaim arm has its own message:\n{text}");
        for secret in [
            "FFPzlMOE6pZPHZWKKJOObg",
            ZITADEL_HUMAN_SUB,
            ZITADEL_CLIENT_ID,
            "58e866bad23abebb",
            "V1_393381929921019907",
            "alice@example.com",
        ] {
            assert!(!text.contains(secret), "the log must not contain {secret:?}:\n{text}");
        }
    }

    #[tokio::test]
    async fn missing_claim_shares_the_rate_limit_of_the_marker_refusals() {
        // Review Focus 1 (D4): the rate-limit key is (issuer, NotAnAccessToken) for steps 6, 6b and
        // 6c. A marker refusal and then a missing-claim refusal within 10 s give ONE line, the
        // first. The runbook tells the operator that one line can stand for many refusals.
        let (logs, _guard) = capture_logs();
        let (encoding_key, jwk, kid) = es256_keypair();
        let authenticator = make_authenticator(
            StubFetcher::new(jwk),
            vec![issuer_with_rules(ISSUER, &[ZITADEL_PROJECT_ID], &ZITADEL_MARKERS, &REQUIRED_JTI)],
            60,
            16_384,
        );
        let id_token = sign(&encoding_key, Some(&kid), &zitadel_m1_id_token());
        assert_not_an_access_token(authenticator.authenticate(&id_token, TokenScheme::Bearer).await, "M1 ID token");
        let no_jti = sign(&encoding_key, Some(&kid), &without(zitadel_m1_access_token(), "jti"));
        assert_not_an_access_token(authenticator.authenticate(&no_jti, TokenScheme::Bearer).await, "access token without jti");
        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|line| line.contains(NOT_ACCESS_TOKEN_REFUSAL) || line.contains(MISSING_CLAIM_REFUSAL)).collect();
        assert_eq!(lines.len(), 1, "one line for the two refusals, got:\n{text}");
        assert!(lines[0].contains("claim at_hash"), "the first refusal is the logged one: {}", lines[0]);
    }

    #[test]
    fn boot_line_names_the_issuer_and_the_required_claims() {
        // SMA-731 T14 (D2): one info line for an issuer with required claims; none for an empty
        // list. The SMA-703 boot line does not show for an empty marker list.
        let (logs, _guard) = capture_logs();
        let (_encoding_key, jwk, _kid) = es256_keypair();
        let _with = make_authenticator(
            StubFetcher::new(jwk.clone()),
            vec![issuer_with_rules(ISSUER, &["aud"], &[], &REQUIRED_JTI), issuer_config(SECOND_ISSUER, &["aud"])],
            60,
            16_384,
        );
        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|line| line.contains(REQUIRED_BOOT_LINE)).collect();
        assert_eq!(lines.len(), 1, "exactly one boot line expected, got:\n{text}");
        let line = lines[0];
        assert!(line.contains("INFO"), "the boot line logs at info: {line}");
        assert!(line.contains(ISSUER), "the boot line names the issuer: {line}");
        assert!(line.contains("jti"), "the boot line names the claims: {line}");
        assert!(!line.contains(SECOND_ISSUER), "the boot line names only the issuer with claims: {line}");
        assert!(!text.contains(MARKER_BOOT_LINE), "no marker boot line for an empty marker list:\n{text}");

        let (logs, _guard) = capture_logs();
        let _without = make_authenticator(StubFetcher::new(jwk), vec![issuer_config(ISSUER, &["aud"])], 60, 16_384);
        assert!(!logs.text().contains(REQUIRED_BOOT_LINE), "no boot line for an empty list:\n{}", logs.text());
    }

    #[tokio::test]
    async fn dpop_scheme_refuses_a_bound_token_without_the_required_claim() {
        // SMA-731 T14a (D3): the check is in `authenticate`, so it applies to the Dpop scheme too.
        // The validator does not see the proof: the caller checks it after `authenticate` returns
        // the binding (`application::dpop`), so a refusal here comes before any proof check.
        let bound = merged(zitadel_m1_access_token(), serde_json::json!({ "cnf": { "jkt": JKT } }));
        assert_not_an_access_token(
            authenticate_rules_as(&[], &REQUIRED_JTI, &without(bound.clone(), "jti"), TokenScheme::Dpop).await,
            "bound token without jti",
        );
        let validated = authenticate_rules_as(&[], &REQUIRED_JTI, &bound, TokenScheme::Dpop)
            .await
            .expect("a bound token with jti passes the Dpop scheme");
        assert_eq!(validated.key_binding, Some(Jkt::new(JKT)));
    }
}
```

- [ ] **Step 2: Run the tests and see the right ones fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims/rs
cargo nextest run -p paigasus-iam --lib --locked --no-fail-fast -E 'test(/validator::tests|config::tests/)'
```

Expected (MEASURED during planning): the build succeeds, and exactly these 12 tests FAIL:
`boot_line_names_the_issuer_and_the_required_claims`,
`dpop_scheme_refuses_a_bound_token_without_the_required_claim`,
`duplicate_required_claim_member_is_malformed`,
`missing_claim_refusal_logs_its_own_message_issuer_and_name_only`,
`missing_claim_shares_the_rate_limit_of_the_marker_refusals`,
`null_required_claim_is_missing_and_any_other_value_is_present`,
`required_claim_names_are_case_sensitive`,
`required_claim_runs_before_the_sender_constraint_check`,
`required_claim_refuses_zitadel_id_tokens`, `required_claims_are_per_issuer`,
`the_first_missing_required_claim_is_logged`,
`unread_duplicate_member_is_malformed_once_a_required_claim_is_set`.
The other new tests pass already, because they pin behaviour that must not change (T2, T6, T7,
T8, T9). Every old test passes.

- [ ] **Step 3: Write the `ClaimRules` unit tests (T14b)**

`ClaimRules` is a new private type, so these tests cannot compile before it exists. Edit
`old_string` (the end of the file after Step 1):

```rust
        assert_eq!(validated.key_binding, Some(Jkt::new(JKT)));
    }
}
```

`new_string`:

```rust
        assert_eq!(validated.key_binding, Some(Jkt::new(JKT)));
    }

    fn claim_rules(markers: &[&str], required: &[&str]) -> ClaimRules {
        ClaimRules {
            id_token_markers: markers.iter().map(|name| (*name).to_string()).collect(),
            required: required.iter().map(|name| (*name).to_string()).collect(),
        }
    }

    fn members(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        value.as_object().expect("members are a JSON object").clone()
    }

    #[test]
    fn claim_rules_need_a_strict_decode_when_either_list_is_set() {
        // SMA-731 T14b (D3).
        assert!(!claim_rules(&[], &[]).needs_strict_decode());
        assert!(claim_rules(&["at_hash"], &[]).needs_strict_decode());
        assert!(claim_rules(&[], &["jti"]).needs_strict_decode());
        assert!(claim_rules(&["at_hash"], &["jti"]).needs_strict_decode());
    }

    #[test]
    fn claim_rules_refusal_checks_markers_first_then_the_required_names() {
        // SMA-731 T14b (D3): marker-first order, the null rule and case-sensitivity, on a plain map.
        let rules = claim_rules(&["at_hash", "azp"], &["jti", "nbf"]);
        let cases = [
            ("a marker wins over a missing name", serde_json::json!({ "at_hash": "x" }), Some(RefusalDetail::Claim("at_hash"))),
            (
                "the first configured marker",
                serde_json::json!({ "azp": "c", "at_hash": "x", "jti": "j", "nbf": 1 }),
                Some(RefusalDetail::Claim("at_hash")),
            ),
            ("the first missing name", serde_json::json!({}), Some(RefusalDetail::MissingClaim("jti"))),
            ("the second missing name", serde_json::json!({ "jti": "j" }), Some(RefusalDetail::MissingClaim("nbf"))),
            ("a null marker is absent", serde_json::json!({ "at_hash": null, "jti": "j", "nbf": 1 }), None),
            (
                "a null required claim is missing",
                serde_json::json!({ "jti": null, "nbf": 1 }),
                Some(RefusalDetail::MissingClaim("jti")),
            ),
            ("any other value is present", serde_json::json!({ "jti": "", "nbf": false }), None),
            (
                "names are case-sensitive",
                serde_json::json!({ "AT_HASH": "x", "JTI": "j", "nbf": 1 }),
                Some(RefusalDetail::MissingClaim("jti")),
            ),
        ];
        for (name, value, want) in cases {
            assert_eq!(rules.refusal(&members(value)), want, "{name}");
        }
        assert_eq!(claim_rules(&[], &[]).refusal(&members(serde_json::json!({ "at_hash": "x" }))), None, "no rules, no refusal");
    }
}
```

- [ ] **Step 4: Run and see the build fail**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims/rs
cargo nextest run -p paigasus-iam --lib --locked -E 'test(/validator::tests/)'
```

Expected: ``error[E0425]: cannot find type `ClaimRules` in this scope``,
``error[E0422]: cannot find struct, variant or union type `ClaimRules` in this scope`` and four
``error[E0599]: no variant or associated item named `MissingClaim` found``. No test runs.

- [ ] **Step 5: Update the module doc**

Edit `old_string`:

```rust
//! `id_token_marker_claims` (SMA-703); for such an issuer the payload is also decoded as a map
//! that refuses a repeated top-level member (`StrictPayload`). The
//! sender-constraint check (SMA-690) refuses a token bound to a key (a `cnf` claim, or a Keycloak
```

`new_string`:

```rust
//! `id_token_marker_claims` (SMA-703). Then it refuses a token that does not carry a claim the
//! operator named for the issuer in `access_token_required_claims` (SMA-731). For an issuer with
//! either list, the payload is also decoded as a map that refuses a repeated top-level member
//! (`StrictPayload`); `ClaimRules` holds the two lists. The
//! sender-constraint check (SMA-690) refuses a token bound to a key (a `cnf` claim, or a Keycloak
```

Edit `old_string`:

```rust
//! Four refusals are logged, rate-limited (`log_refusal`). Never logs token or claim material (`TokenDefect` itself carries
//! no payload).
```

`new_string`:

```rust
//! Four defects are logged, rate-limited (`log_refusal`); `NotAnAccessToken` has two messages, one
//! for a marker and one for a missing required claim. Never logs token or claim material
//! (`TokenDefect` itself carries no payload).
```

- [ ] **Step 6: Extend `RefusalDetail`, and add `ClaimRules` to `ConfiguredIssuer`**

Edit `old_string`:

```rust
/// What a refusal log line names besides the issuer (SMA-686 D8, D11). Static or configured
/// values only — never a token claim.
enum RefusalDetail<'a> {
```

`new_string`:

```rust
/// What a refusal log line names besides the issuer (SMA-686 D8, D11). Static or configured
/// values only — never a token claim. `Debug` and `PartialEq` serve the `ClaimRules` unit tests.
#[derive(Debug, PartialEq, Eq)]
enum RefusalDetail<'a> {
```

Edit `old_string`:

```rust
    /// D4). Logged as the marker `claim <name>`.
    Claim(&'a str),
```

`new_string`:

```rust
    /// D4). Logged as the marker `claim <name>`.
    Claim(&'a str),
    /// The CONFIGURED claim name that a verified token does not carry (SMA-731 D4). Logged as the
    /// marker `missing claim <name>`, with its own message.
    MissingClaim(&'a str),
```

Edit `old_string`:

```rust
struct ConfiguredIssuer {
    issuer: Issuer,
    audiences: Vec<String>,
    /// SMA-703 D1: the configured claim names. Empty keeps the SMA-686 decode path unchanged.
    id_token_marker_claims: Vec<String>,
}
```

`new_string`:

```rust
struct ConfiguredIssuer {
    issuer: Issuer,
    audiences: Vec<String>,
    /// SMA-703 D1 and SMA-731 D1: the configured claim lists. Both empty keep the SMA-686 decode
    /// path unchanged.
    claim_rules: ClaimRules,
}

/// The configured claim rules of one issuer (SMA-731 D3): the SMA-703 marker claims and the
/// SMA-731 required claims. `authenticate` asks it two things: which decode to run, and whether
/// the verified payload is refused. The request path does not read `IssuerConfig` again.
struct ClaimRules {
    /// `id_token_marker_claims`: a token that carries one of them is refused (step 6b).
    id_token_markers: Vec<String>,
    /// `access_token_required_claims`: a token that does not carry one of them is refused (step 6c).
    required: Vec<String>,
}

impl ClaimRules {
    /// True when either list is not empty. Then the payload is decoded as a `StrictPayload`, so a
    /// repeated top-level member is `Malformed` (SMA-703 D3). Both empty keep the plain decode (G2).
    fn needs_strict_decode(&self) -> bool {
        !self.id_token_markers.is_empty() || !self.required.is_empty()
    }

    /// Step 6b, then step 6c, on the verified top-level members: `Claim` for the first configured
    /// marker that is present, else `MissingClaim` for the first required name that is missing,
    /// else `None`. A claim is present when its member exists and is not JSON `null`.
    fn refusal(&self, members: &serde_json::Map<String, serde_json::Value>) -> Option<RefusalDetail<'_>> {
        if let Some(name) = configured_marker(members, &self.id_token_markers) {
            return Some(RefusalDetail::Claim(name));
        }
        missing_required_claim(members, &self.required).map(RefusalDetail::MissingClaim)
    }
}
```

- [ ] **Step 7: Build `ClaimRules` in `new`, and write the boot line**

Edit `old_string`:

```rust
    /// Writes one `info` line for each issuer with configured `id_token_marker_claims` (SMA-703
    /// D2). `AppState::new` calls this once at boot, after `paigasus_logging::init`;
```

`new_string`:

```rust
    /// Writes one `info` line for each issuer with configured `id_token_marker_claims` (SMA-703
    /// D2), and one for each issuer with configured `access_token_required_claims` (SMA-731 D2).
    /// `AppState::new` calls this once at boot, after `paigasus_logging::init`;
```

Edit `old_string`:

```rust
                Ok(ConfiguredIssuer {
                    issuer,
                    audiences: cfg.audiences,
                    id_token_marker_claims: cfg.id_token_marker_claims,
                })
```

`new_string`:

```rust
                if !cfg.access_token_required_claims.is_empty() {
                    tracing::info!(
                        issuer = issuer.as_str(),
                        access_token_required_claims = ?cfg.access_token_required_claims,
                        "IAM refuses a verified token of this issuer that does not carry every configured required claim"
                    );
                }
                Ok(ConfiguredIssuer {
                    issuer,
                    audiences: cfg.audiences,
                    claim_rules: ClaimRules {
                        id_token_markers: cfg.id_token_marker_claims,
                        required: cfg.access_token_required_claims,
                    },
                })
```

- [ ] **Step 8: Log the `MissingClaim` refusal with its own message**

Edit `old_string`:

```rust
    /// The one place that decides which refusals are logged (SMA-686 D8, D11, D14, D15; SMA-690
    /// D8; SMA-700 § 4.3): only `NotAnAccessToken`, `AudienceMismatch`, `SenderConstrained` and `NotKeyBound`, each reachable
    /// only for a correctly signed token from a configured issuer, and each rate-limited per
    /// (issuer, defect). Logs the issuer and a static or configured detail — never a token claim.
```

`new_string`:

```rust
    /// The one place that decides which refusals are logged (SMA-686 D8, D11, D14, D15; SMA-690
    /// D8; SMA-700 § 4.3): only `NotAnAccessToken`, `AudienceMismatch`, `SenderConstrained` and `NotKeyBound`, each reachable
    /// only for a correctly signed token from a configured issuer, and each rate-limited per
    /// (issuer, defect). Logs the issuer and a static or configured detail — never a token claim.
    /// A missing required claim (SMA-731 D4) is a `NotAnAccessToken` refusal with its own
    /// message; it shares the rate limit of the other `NotAnAccessToken` refusals of the issuer.
```

Edit `old_string`:

```rust
                tracing::info!(issuer = issuer.as_str(), marker = marker.as_str(), suppressed, "{}", NOT_AN_ACCESS_TOKEN_MESSAGE);
            }
            RefusalDetail::NotKeyBound => {
```

`new_string`:

```rust
                tracing::info!(issuer = issuer.as_str(), marker = marker.as_str(), suppressed, "{}", NOT_AN_ACCESS_TOKEN_MESSAGE);
            }
            RefusalDetail::MissingClaim(name) => {
                // Its own message (SMA-731 D4): the refused token can be a real access token after
                // an IdP change, so "not an access token" would point the operator at the client.
                let marker = format!("missing claim {name}");
                tracing::info!(issuer = issuer.as_str(), marker = marker.as_str(), suppressed, "{}", MISSING_CLAIM_MESSAGE);
            }
            RefusalDetail::NotKeyBound => {
```

Edit `old_string`:

```rust
/// The log message of a `NotAnAccessToken` refusal. The `Marker` and `Claim` arms of
/// `log_refusal` both use it, so an operator greps one text.
const NOT_AN_ACCESS_TOKEN_MESSAGE: &str = "refused a bearer token: a verified marker shows it is not an access token";
```

`new_string`:

```rust
/// The log message of a `NotAnAccessToken` refusal. The `Marker` and `Claim` arms of
/// `log_refusal` both use it, so an operator greps one text. The `MissingClaim` arm uses
/// `MISSING_CLAIM_MESSAGE` (SMA-731 D4).
const NOT_AN_ACCESS_TOKEN_MESSAGE: &str = "refused a bearer token: a verified marker shows it is not an access token";

/// The log message of a `NotAnAccessToken` refusal for a missing required claim (SMA-731 D4).
const MISSING_CLAIM_MESSAGE: &str = "refused a bearer token: it does not carry a claim that the issuer configuration requires";
```

- [ ] **Step 9: Add `missing_required_claim`**

Edit `old_string`:

```rust
fn configured_marker<'a>(members: &serde_json::Map<String, serde_json::Value>, names: &'a [String]) -> Option<&'a str> {
    names.iter().find(|name| members.get(name.as_str()).is_some_and(|value| !value.is_null())).map(String::as_str)
}
```

`new_string`:

```rust
fn configured_marker<'a>(members: &serde_json::Map<String, serde_json::Value>, names: &'a [String]) -> Option<&'a str> {
    names.iter().find(|name| members.get(name.as_str()).is_some_and(|value| !value.is_null())).map(String::as_str)
}

/// The first configured required claim name that the verified payload does not carry, or `None`
/// (SMA-731 D3). A member with the value JSON `null` counts as missing. Any other value counts as
/// present: a string, a number, `false`, an object, an array, an empty string. Names compare
/// exactly. The list order decides which name a log line shows.
fn missing_required_claim<'a>(members: &serde_json::Map<String, serde_json::Value>, names: &'a [String]) -> Option<&'a str> {
    names.iter().find(|name| members.get(name.as_str()).is_none_or(serde_json::Value::is_null)).map(String::as_str)
}
```

- [ ] **Step 10: Use `ClaimRules` in `authenticate` (one branch for steps 6b and 6c)**

Edit `old_string`:

```rust
        // SMA-703 D3: an issuer with no marker claims keeps the SMA-686 decode exactly (G2). One
        // with marker claims decodes the same verified bytes as a `StrictPayload`; the signature
        // is checked once either way.
        let (verified_header, claims, claim_marker) = if issuer_config.id_token_marker_claims.is_empty() {
            let token_data = decode::<WireClaims>(token, &decoding_key, &validation).map_err(on_decode_error)?;
            (token_data.header, token_data.claims, None)
        } else {
            let token_data = decode::<StrictPayload>(token, &decoding_key, &validation).map_err(on_decode_error)?;
            let marker = configured_marker(&token_data.claims.members, &issuer_config.id_token_marker_claims);
            (token_data.header, token_data.claims.claims, marker)
        };
```

`new_string`:

```rust
        // SMA-703 D3 and SMA-731 D3: an issuer with no claim rules keeps the SMA-686 decode
        // exactly (G2). One with a marker list or a required list decodes the same verified bytes
        // as a `StrictPayload`; the signature is checked once either way.
        let (verified_header, claims, claim_refusal) = if !issuer_config.claim_rules.needs_strict_decode() {
            let token_data = decode::<WireClaims>(token, &decoding_key, &validation).map_err(on_decode_error)?;
            (token_data.header, token_data.claims, None)
        } else {
            let token_data = decode::<StrictPayload>(token, &decoding_key, &validation).map_err(on_decode_error)?;
            let refusal = issuer_config.claim_rules.refusal(&token_data.claims.members);
            (token_data.header, token_data.claims.claims, refusal)
        };
```

Edit `old_string`:

```rust
        // 6b. A configured marker claim (SMA-703 D3), after the SMA-686 markers.
        if let Some(name) = claim_marker {
            self.log_refusal(&issuer, TokenDefect::NotAnAccessToken, RefusalDetail::Claim(name));
            return Err(invalid(TokenDefect::NotAnAccessToken));
        }
```

`new_string`:

```rust
        // 6b. A configured marker claim (SMA-703 D3), then 6c. a missing required claim (SMA-731
        // D3), after the SMA-686 markers and before the key binding. `ClaimRules::refusal` keeps
        // the 6b-then-6c order.
        if let Some(detail) = claim_refusal {
            self.log_refusal(&issuer, TokenDefect::NotAnAccessToken, detail);
            return Err(invalid(TokenDefect::NotAnAccessToken));
        }
```

- [ ] **Step 11: Update the `TokenDefect::NotAnAccessToken` doc (spec D4)**

In `rs/crates/libs/paigasus-iam-core/src/authn.rs`, Edit `old_string`:

```rust
    /// The payload `typ` claim marks the token as a Keycloak ID token or back-channel logout
    /// token, not an access token (SMA-686).
    NotAnAccessToken,
```

`new_string`:

```rust
    /// The verified token is not an access token, or it does not carry a claim that the issuer
    /// configuration requires of an access token. Three checks give this defect: a payload `typ`
    /// of a Keycloak ID token or logout token, or a back-channel logout marker (SMA-686); a claim
    /// named in the issuer's `id_token_marker_claims` (SMA-703); a missing claim named in the
    /// issuer's `access_token_required_claims` (SMA-731).
    NotAnAccessToken,
```

- [ ] **Step 12: Run the tests and see them pass**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims/rs
cargo nextest run -p paigasus-iam --lib --locked -E 'test(/validator::tests|config::tests/)'
```

Expected (MEASURED during planning): `188 tests run: 188 passed`. The old SMA-703 tests pass
unchanged, `duplicate_member_is_malformed_on_the_strict_path`,
`duplicate_unknown_member_is_unchanged_on_the_plain_path`,
`strict_path_keeps_the_defect_order` and `boot_line_names_the_issuer_and_the_marker_claims`
included.

- [ ] **Step 13: Run the whole lib suites, fmt and clippy**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims/rs
cargo nextest run -p paigasus-iam --lib --locked
cargo nextest run -p paigasus-iam-core --locked
cargo fmt --check -p paigasus-iam -p paigasus-iam-core
cargo clippy -p paigasus-iam -p paigasus-iam-core --all-targets --locked -- -D warnings
```

Expected (MEASURED during planning): 974 `paigasus-iam` lib tests and 136 `paigasus-iam-core`
tests PASS; fmt prints nothing; clippy exits 0.

- [ ] **Step 14: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims
git add rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs \
  rs/crates/libs/paigasus-iam-core/src/authn.rs
git commit -m "feat(rs): IAM refuses a token that lacks a configured required claim (SMA-731)" \
  -m "A new private ClaimRules holds the marker list and the required list of an issuer. It
selects the strict decode when either list is set, and it returns the refusal: a marker claim
first (step 6b), then the first missing required claim (step 6c). The refusal is
NotAnAccessToken. It logs the marker missing claim <name> with its own message. IAM logs the
required names once at boot. The TokenDefect doc names all three NotAnAccessToken checks." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git log --oneline -1
```

---

### Task 4: `zitadel_e2e.rs` proves the required claim against a real Zitadel (T18, T19, T19a, T21)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/zitadel_e2e.rs` (module doc `:18-19`, constant `:72`, T18 after the F6 loops `:236`, T19 and T19a at the end of the test `:336`, `zitadel_config` `:727`)

**Interfaces:**
- Consumes: `IssuerConfig::access_token_required_claims` (Task 2); the validator behaviour of Task 3; the existing helpers `has_claim`, `jwt_payload`, `send`, `router`, `AppState::new`.
- Produces: `const REQUIRED_CLAIMS: [&str; 1]` and `fn zitadel_config_with_rules(issuer: &str, project_id: &str, markers: &[&str], required: &[&str]) -> IamConfig`. `zitadel_config(issuer, project_id, markers)` keeps its signature and delegates, so every existing call site and assertion stays unchanged (T21).

- [ ] **Step 1: Add the module doc, the constant and the T18 shape assertions**

Edit `old_string`:

```rust
//! - A control: with an EMPTY list, IAM does not refuse any of the three ID tokens as
//!   `NotAnAccessToken`. So the setting, not another check, does the refusal.
```

`new_string`:

```rust
//! - A control: with an EMPTY list, IAM does not refuse any of the three ID tokens as
//!   `NotAnAccessToken`. So the setting, not another check, does the refusal.
//!
//! SMA-731 adds, for the same tokens:
//! - `jti` is on every access token and on no ID token (SMA-731 spec § 3, F1).
//! - IAM with `access_token_required_claims = ["jti"]` and an EMPTY marker list refuses each ID
//!   token as `NotAnAccessToken`, so the required claim refuses them by itself. The access tokens
//!   still pass.
//! - IAM with the full runbook recipe (both settings) refuses each ID token and passes each
//!   access token.
```

Edit `old_string`:

```rust
/// The IAM setting under test (spec D1, the runbook recipe).
const MARKER_CLAIMS: [&str; 2] = ["at_hash", "azp"];
```

`new_string`:

```rust
/// The IAM setting under test (spec D1, the runbook recipe).
const MARKER_CLAIMS: [&str; 2] = ["at_hash", "azp"];
/// The SMA-731 setting under test (SMA-731 spec D6, the runbook recipe).
const REQUIRED_CLAIMS: [&str; 1] = ["jti"];
```

Edit `old_string`:

```rust
        assert!(aud_contains(&claims, &setup.project_id), "{label} aud must contain the project id: {claims}");
    }
```

`new_string`:

```rust
        assert!(aud_contains(&claims, &setup.project_id), "{label} aud must contain the project id: {claims}");
    }
    // SMA-731 F1 (T18): `jti` on every access token, on no ID token.
    for (label, token) in id_tokens {
        let claims = jwt_payload(token);
        for name in REQUIRED_CLAIMS {
            assert!(!has_claim(&claims, name), "{label} must NOT carry {name} (SMA-731 F1): {claims}");
        }
    }
    for (label, token) in access_tokens {
        let claims = jwt_payload(token);
        for name in REQUIRED_CLAIMS {
            assert!(has_claim(&claims, name), "{label} must carry {name} (SMA-731 F1): {claims}");
        }
    }
```

- [ ] **Step 2: Add the T19 and T19a blocks at the end of the test**

Edit `old_string`:

```rust
        "with an empty marker list, the machine ID token must fail JIT only for the missing email, got {err:?}"
    );
}
```

`new_string`:

```rust
        "with an empty marker list, the machine ID token must fail JIT only for the missing email, got {err:?}"
    );

    // --- SMA-731: IAM with the required claim ---

    // T19: `["jti"]` with an EMPTY marker list. Each ID token is refused by step 6c alone, and
    // the wire answer is the same 401 `invalid-token` (Review Focus 5). The human and the
    // refreshed access token resolve to the human principal. The machine access token passes the
    // authenticator and fails JIT only for the missing email (SMA-703 § 12).
    let required_cfg = zitadel_config_with_rules(&issuer, &setup.project_id, &[], &REQUIRED_CLAIMS);
    let required_state = AppState::new(state.db.clone(), &required_cfg).await.expect("AppState::new (required claims)");
    let required_app = router(required_state.clone());
    for (label, token) in id_tokens {
        let err = required_state.authn.resolve(token, Provisioning::Disabled).await.expect_err("an ID token must not authenticate");
        assert!(
            matches!(err, AuthnError::InvalidToken(TokenDefect::NotAnAccessToken)),
            "with the required claim only, the {label} must be refused as NotAnAccessToken, got {err:?}"
        );
        let (status, body) = send(&required_app, "POST", "/v1/organizations", Some(json!({ "slug": "nojti", "name": "No jti" })), Some(token)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{label}: {body}");
        assert_eq!(body["error"]["code"], "invalid-token", "{label}: {body}");
    }
    for (label, token) in [("human access token", &human_access), ("refreshed access token", &refreshed_access)] {
        let principal = required_state
            .authn
            .resolve(token, Provisioning::Disabled)
            .await
            .unwrap_or_else(|err| panic!("with the required claim, IAM must accept the {label}, got {err:?}"));
        assert_eq!(principal.principal_id.canonical(), principal_prn, "the {label} must resolve to the human principal");
    }
    let err = required_state.authn.resolve(&machine_access, Provisioning::Enabled).await.expect_err("JIT needs an email");
    assert!(
        matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail)),
        "with the required claim, the machine access token must fail JIT only for the missing email, got {err:?}"
    );

    // T19a: the full runbook recipe, the marker claims and the required claim together.
    let recipe_cfg = zitadel_config_with_rules(&issuer, &setup.project_id, &MARKER_CLAIMS, &REQUIRED_CLAIMS);
    let recipe_state = AppState::new(state.db.clone(), &recipe_cfg).await.expect("AppState::new (full recipe)");
    for (label, token) in id_tokens {
        let err = recipe_state.authn.resolve(token, Provisioning::Disabled).await.expect_err("an ID token must not authenticate");
        assert!(
            matches!(err, AuthnError::InvalidToken(TokenDefect::NotAnAccessToken)),
            "with the full recipe, the {label} must be refused as NotAnAccessToken, got {err:?}"
        );
    }
    for (label, token) in [("human access token", &human_access), ("refreshed access token", &refreshed_access)] {
        let principal = recipe_state
            .authn
            .resolve(token, Provisioning::Disabled)
            .await
            .unwrap_or_else(|err| panic!("with the full recipe, IAM must accept the {label}, got {err:?}"));
        assert_eq!(principal.principal_id.canonical(), principal_prn, "the {label} must resolve to the human principal");
    }
    let err = recipe_state.authn.resolve(&machine_access, Provisioning::Enabled).await.expect_err("JIT needs an email");
    assert!(
        matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail)),
        "with the full recipe, the machine access token must fail JIT only for the missing email, got {err:?}"
    );
}
```

- [ ] **Step 3: Run and see the build fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims/rs
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test zitadel_e2e
```

Expected: ``error[E0425]: cannot find function `zitadel_config_with_rules` in this scope``.
(Tasks 2 and 3 are in place, so the red is the missing helper only. The behaviour itself is
proved red by the Task 6 mutation M1.)

- [ ] **Step 4: Split `zitadel_config` and pass the required list**

Edit `old_string`:

```rust
fn zitadel_config(issuer: &str, project_id: &str, markers: &[&str]) -> IamConfig {
    IamConfig {
```

`new_string`:

```rust
fn zitadel_config(issuer: &str, project_id: &str, markers: &[&str]) -> IamConfig {
    zitadel_config_with_rules(issuer, project_id, markers, &[])
}

/// `zitadel_config` with the required claims `required` too (SMA-731 T19, T19a).
fn zitadel_config_with_rules(issuer: &str, project_id: &str, markers: &[&str], required: &[&str]) -> IamConfig {
    IamConfig {
```

Edit `old_string`:

```rust
                id_token_marker_claims: markers.iter().map(|name| (*name).to_string()).collect(),
                access_token_required_claims: Vec::new(),
```

`new_string`:

```rust
                id_token_marker_claims: markers.iter().map(|name| (*name).to_string()).collect(),
                access_token_required_claims: required.iter().map(|name| (*name).to_string()).collect(),
```

- [ ] **Step 5: Run the Zitadel suite and see it pass**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims/rs
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test zitadel_e2e
cargo fmt --check -p paigasus-iam
cargo clippy -p paigasus-iam --all-targets --locked -- -D warnings
```

Expected (MEASURED during planning): `zitadel_id_tokens_are_refused_by_the_marker_claims` PASSES
in about 8 s on an idle Docker Desktop; fmt prints nothing; clippy exits 0. T21: `git diff` of
this task shows no change to an SMA-703 assertion line.

- [ ] **Step 6: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims
git add rs/crates/services/paigasus-iam/tests/zitadel_e2e.rs
git commit -m "test(rs): zitadel_e2e proves the required claim jti against Zitadel v4.15.3 (SMA-731)" \
  -m "The test pins jti on every access token and on no ID token. With the required claim and an
empty marker list, IAM refuses each ID token as NotAnAccessToken with a 401 invalid-token, and
the access tokens still pass. The full runbook recipe gives the same result. The SMA-703
assertions do not change." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git log --oneline -1
```

---

### Task 5: `keycloak_e2e.rs` runs IAM with `["jti"]` (T20 part 2, branch A ONLY)

**Run this task only if Task 1 Step 3 showed the split (branch A).** The planning measurement is
branch B (no split). For branch B, spec D8 says: "If the split does not hold, the test asserts the
measured presence only." Task 1 already does that. Then skip this task, write
"Task 5 skipped: branch B (Keycloak 26.4 has jti on every token, SMA-731 F4)" in the task tracker,
and go to Task 6.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs` (end of the test, `keycloak_config`)

**Interfaces:**
- Consumes: Task 1 locals `refreshed_access`, `dpop_key`, `dpop_x`, `dpop_y`, `dpop_token`, `principal_prn`; Task 2 field; Task 3 behaviour.
- Produces: `fn keycloak_config_with_required(issuer: &str, required: &[&str]) -> IamConfig`.

- [ ] **Step 1: Add the T20 IAM run at the end of the test**

Edit `old_string`:

```rust
    let err = state.authn.introspect_dpop(&dpop_token, request).await.expect_err("the same proof twice is a replay");
    assert!(matches!(err, AuthnError::InvalidDpopProof(ProofDefect::Replayed)), "got {err:?}");
}
```

`new_string`:

```rust
    let err = state.authn.introspect_dpop(&dpop_token, request).await.expect_err("the same proof twice is a replay");
    assert!(matches!(err, AuthnError::InvalidDpopProof(ProofDefect::Replayed)), "got {err:?}");

    // SMA-731 T20: IAM with `access_token_required_claims = ["jti"]`. The ID token is refused,
    // but by the `typ` check (step 6) before step 6c, so this does NOT prove step 6c (spec D8).
    // The password, the refreshed and the DPoP-bound access token (with a new valid proof) pass.
    let required_cfg = keycloak_config_with_required(&issuer, &["jti"]);
    let required_state = AppState::new(state.db.clone(), &required_cfg).await.expect("AppState::new (required claims)");
    let err = required_state.authn.resolve(&id_token, Provisioning::Disabled).await.expect_err("an ID token must not authenticate");
    assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::NotAnAccessToken)), "got {err:?}");
    for (label, token) in [("password access token", &access_token), ("refreshed access token", &refreshed_access)] {
        let principal = required_state
            .authn
            .resolve(token, Provisioning::Disabled)
            .await
            .unwrap_or_else(|err| panic!("with the required claim, IAM must accept the {label}, got {err:?}"));
        assert_eq!(principal.principal_id.canonical(), principal_prn, "the {label} must resolve to alice");
    }
    let proof = dpop_proof(&dpop_key, &dpop_x, &dpop_y, "POST", "https://gw.example.test/v1/chat/completions", Some(&dpop_token));
    let request = DpopRequest {
        proof,
        method: "POST".to_string(),
        path: "/v1/chat/completions".to_string(),
    };
    let ctx = required_state
        .authn
        .introspect_dpop(&dpop_token, request)
        .await
        .expect("with the required claim, the DPoP-bound token with a valid proof passes Introspect");
    assert_eq!(ctx.principal.principal_id.canonical(), principal_prn, "the bound token resolves to alice");
}
```

- [ ] **Step 2: Run and see the build fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims/rs
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test keycloak_e2e
```

Expected: ``error[E0425]: cannot find function `keycloak_config_with_required` in this scope``.

- [ ] **Step 3: Split `keycloak_config`**

Edit `old_string`:

```rust
fn keycloak_config(issuer: &str) -> IamConfig {
    IamConfig {
```

`new_string`:

```rust
fn keycloak_config(issuer: &str) -> IamConfig {
    keycloak_config_with_required(issuer, &[])
}

/// `keycloak_config` with the required claims `required` (SMA-731 T20).
fn keycloak_config_with_required(issuer: &str, required: &[&str]) -> IamConfig {
    IamConfig {
```

Edit `old_string`:

```rust
                id_token_marker_claims: Vec::new(),
                access_token_required_claims: Vec::new(),
```

`new_string`:

```rust
                id_token_marker_claims: Vec::new(),
                access_token_required_claims: required.iter().map(|name| (*name).to_string()).collect(),
```

- [ ] **Step 4: Run and see it pass**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims/rs
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test keycloak_e2e
cargo fmt --check -p paigasus-iam
cargo clippy -p paigasus-iam --all-targets --locked -- -D warnings
```

Expected: PASS; fmt prints nothing; clippy exits 0.

- [ ] **Step 5: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims
git add rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs
git commit -m "test(rs): keycloak_e2e runs IAM with the required claim jti (SMA-731)" \
  -m "With access_token_required_claims set to jti, the password, the refreshed and the
DPoP-bound access token still pass. The ID token is refused by the typ check first, so this
test does not claim to prove step 6c." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git log --oneline -1
```

---

### Task 6: Mutation proof (spec § 5)

**Files:**
- Modify (temporarily, restored in this task): `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs`

**Interfaces:**
- Consumes: Tasks 3 and 4.
- Produces: nothing in the tree. The task tracker gets one line for each mutation with the real failing test names.

Rules for this task:
- Make each mutation with the Edit tool. Restore it with the Edit tool (swap `old_string` and
  `new_string`). Do NOT use `git checkout --`, `git stash` or `git restore`.
- The spec says "delete step 6c". A deletion of the `missing_required_claim(…)` expression leaves
  `missing_required_claim` and `RefusalDetail::MissingClaim` unused, and `warnings = "deny"` then
  stops the build at rustc. That proves nothing. So M1 keeps every item in use and turns the
  branch off with `.filter(|_| false)`. Confirm for each mutation that the run reaches
  `Summary … tests run`, not `error[E…]`.

- [ ] **Step 1: M1 — step 6c returns nothing (the spec mutation)**

Edit `old_string`:

```rust
        missing_required_claim(members, &self.required).map(RefusalDetail::MissingClaim)
```

`new_string`:

```rust
        missing_required_claim(members, &self.required).map(RefusalDetail::MissingClaim).filter(|_| false)
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims/rs
cargo nextest run -p paigasus-iam --lib --locked --no-fail-fast -E 'test(/validator::tests/)'
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test zitadel_e2e --no-fail-fast
```

Expected (MEASURED during planning): the lib run compiles and 10 tests FAIL:
`required_claim_refuses_zitadel_id_tokens` (T1), `the_first_missing_required_claim_is_logged`
(T4), `dpop_scheme_refuses_a_bound_token_without_the_required_claim` (T14a),
`null_required_claim_is_missing_and_any_other_value_is_present`,
`required_claim_names_are_case_sensitive`, `required_claim_runs_before_the_sender_constraint_check`,
`required_claims_are_per_issuer`, `missing_claim_refusal_logs_its_own_message_issuer_and_name_only`,
`missing_claim_shares_the_rate_limit_of_the_marker_refusals`,
`claim_rules_refusal_checks_markers_first_then_the_required_names`. The `zitadel_e2e` run FAILS
(T19) with a panic `an ID token must not authenticate` in the T19 loop (nextest retries it once;
both tries fail). Restore M1 with the Edit tool.

- [ ] **Step 2: M2 — a `null` required claim counts as present**

Edit `old_string`:

```rust
members.get(name.as_str()).is_none_or(serde_json::Value::is_null)
```

`new_string`:

```rust
members.get(name.as_str()).is_none()
```

Run the lib command of Step 1. Expected: 2 tests FAIL:
`null_required_claim_is_missing_and_any_other_value_is_present` and
`claim_rules_refusal_checks_markers_first_then_the_required_names`. Restore M2.

- [ ] **Step 3: M3 — the required list alone keeps the plain decode**

Edit `old_string`:

```rust
        !self.id_token_markers.is_empty() || !self.required.is_empty()
```

`new_string`:

```rust
        !self.id_token_markers.is_empty()
```

Run the lib command of Step 1. Expected: 11 tests FAIL, among them
`claim_rules_need_a_strict_decode_when_either_list_is_set`,
`duplicate_required_claim_member_is_malformed` (T11) and
`unread_duplicate_member_is_malformed_once_a_required_claim_is_set`. Restore M3.

- [ ] **Step 4: Prove the tree is clean again**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims/rs
cargo nextest run -p paigasus-iam --lib --locked -E 'test(/validator::tests/)'
cd ..
git status --short
```

Expected: every validator test PASSES; `git status --short` prints nothing (no change since the
last task commit). If a test name differs from the lists above, record the real names. A mutation
that reds no test is a finding: stop and report it. No commit in this task.

---

### Task 7: The chart value `oidc.accessTokenRequiredClaims`

**Files:**
- Modify: `charts/paigasus/templates/_iam-backend.tpl` (header doc `:41-43`, calls `:105`, the two marker helpers `:109-167` become four shared helpers)
- Modify: `charts/paigasus/templates/backend-deployment.yaml:129-138`
- Modify: `charts/paigasus/values.yaml:210-211` (new key after the `scopes` block)
- Modify: `charts/paigasus/README.md:265` and a new section before `## DPoP on the gateway path` (`:279`)
- Modify: `charts/paigasus/tests/env.sh` (comments `:702-704`, `:722-724`, the M8 grep `:794`, R rows before the DPoP rows `:812`)
- Modify: `charts/paigasus/tests/refusals.sh` (rows before the SMA-700 rows `:309`)

**Interfaces:**
- Consumes: the env form `access_token_required_claims=["…",…]` that Task 2's figment test parses.
- Produces: Helm helpers `paigasus.iamIssuerClaimList` (dict `root`, `key`, `field`), `paigasus.iamIdTokenMarkerClaims` and `paigasus.iamAccessTokenRequiredClaims` (root context, return the suffix or `""`), `paigasus.validateClaimNameList` (dict `root`, `key`, `path`, `example`, `reserved`), `paigasus.validateClaimListOverlap` (root context). `paigasus.validateIdTokenMarkerClaims` is removed. The YAML comment line `            # oidc.accessTokenRequiredClaims is set: IAM refuses a token that lacks one of these claims.`

- [ ] **Step 1: Write the failing `env.sh` rows**

In `charts/paigasus/tests/env.sh`, anchor the M8 setup grep to the key line. The new
`values.yaml` comment names `oidc.idTokenMarkerClaims` (spec D5), and the old grep matches any
line (MEASURED during planning). Edit `old_string`:

```bash
if grep -q 'idTokenMarkerClaims' "$TMP/chart-no-markers/values.yaml"; then
```

`new_string`:

```bash
# Match the key line only: the comment of oidc.accessTokenRequiredClaims names this key too (SMA-731).
if grep -q '^  idTokenMarkerClaims:' "$TMP/chart-no-markers/values.yaml"; then
```

Then add the R rows. Edit `old_string`:

```bash
# zones.iam.backend.dpop (SMA-700). Renders go to a file, as for check_boot. One row per property:
```

`new_string`:

```bash
# oidc.accessTokenRequiredClaims (SMA-731). Renders go to a file, as for check_markers. One row per
# property:
#   R1 default      IAM_AUTHN__ISSUERS is the value from before SMA-731, and the
#                   "oidc.accessTokenRequiredClaims is set" comment line is absent.
#   R2 reuse-values-no-key
#                   `--set oidc.accessTokenRequiredClaims=null` on this chart. Helm deletes the key,
#                   and dig gives the default. The value does not change.
#   R3 set          The Zitadel recipe ["jti"], quoted with %q, after the audiences. config.rs
#                   parses this exact form in
#                   issuers_env_in_the_chart_form_parses_access_token_required_claims.
#   R4 both-lists   Both lists set (the runbook recipe for Zitadel). The marker list comes first,
#                   then the required list, and both comment lines render.
#   R5 empty-list-set-json
#                   `--set-json 'oidc.accessTokenRequiredClaims=[]'` renders the default value. It
#                   is the removal form that the runbook gives for --reuse-values.
#   R6 restart-scope
#                   A change of the value changes the IAM pod template and no console pod template.
#   R7 reuse-values-nil-guard
#                   A copy of the chart without the two claim-list keys in values.yaml, and both
#                   values set to null. This is a release from before SMA-703 with --reuse-values.
#                   Helm keeps the nil values, so the row reaches the nil guards of
#                   paigasus.validateClaimNameList and paigasus.validateClaimListOverlap.
#   R8 nil-markers-with-required
#                   The same chart copy, oidc.idTokenMarkerClaims=null and the required list set.
#                   The overlap check reads the nil marker list as [] (Review Focus 4).
# A row counter reds the script when an R row call line is deleted.
REQUIRED_ROWS=0
REQUIRED_ROWS_WANT=8

# check_required <label> <want suffix or -> <marker comment present|absent>
#                <required comment present|absent> [helm args...]
# REQUIRED_CHART, when set, is the chart directory to render instead of $CHART.
check_required() {
  local label="$1" suffix="$2" markers="$3" required="$4"; shift 4
  local out
  REQUIRED_ROWS=$((REQUIRED_ROWS + 1))
  if ! helm template paigasus "${REQUIRED_CHART:-$CHART}" "${BASE[@]+"${BASE[@]}"}" "$@" >"$TMP/required.yaml" 2>"$TMP/required.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/required.err"; ec=1; return 0
  fi
  if ! out="$(SUFFIX="$suffix" MARKERS="$markers" REQUIRED="$required" python3 -c '
import os, sys, yaml
suffix = "" if os.environ["SUFFIX"] == "-" else os.environ["SUFFIX"]
want = "[{issuer=\"https://idp.example.test/realms/paigasus\",audiences=[\"paigasus-console\"]" + suffix + "}]"
comments = {
    "MARKERS": "            # oidc.idTokenMarkerClaims is set: IAM refuses a token that carries one of these claims.",
    "REQUIRED": "            # oidc.accessTokenRequiredClaims is set: IAM refuses a token that lacks one of these claims.",
}
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
for name, line in comments.items():
    count = raw.splitlines().count(line)
    want_count = 1 if os.environ[name] == "present" else 0
    if count != want_count:
        problems.append("the " + name.lower() + " comment line renders " + str(count) + " time(s), want " + str(want_count))
print("|".join(problems) if problems else "OK")' "$TMP/required.yaml" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

# check_required_restart <label>: the value changes the IAM pod template only. It reuses
# check_boot_restart, so it also counts as a B row; the B floor check ran above, as for M6.
check_required_restart() {
  local label="$1"
  REQUIRED_ROWS=$((REQUIRED_ROWS + 1))
  check_boot_restart "$label" --set 'oidc.accessTokenRequiredClaims={jti}'
}

printf 'oidc:\n  idTokenMarkerClaims: ["at_hash", "azp"]\n  accessTokenRequiredClaims: ["jti"]\n' >"$TMP/required-both.yaml"
REQUIRED_SUFFIX=',access_token_required_claims=["jti"]'
BOTH_SUFFIX=',id_token_marker_claims=["at_hash","azp"],access_token_required_claims=["jti"]'

# R7/R8 chart copy: no idTokenMarkerClaims and no accessTokenRequiredClaims key in values.yaml.
# The copy goes in $TMP, which the EXIT trap removes. The script reds when a key stays.
cp -R "$CHART" "$TMP/chart-no-claim-lists"
if ! grep -q '^  accessTokenRequiredClaims:' "$TMP/chart-no-claim-lists/values.yaml"; then
  echo "FAIL [R7 setup]: values.yaml has no accessTokenRequiredClaims line to remove"; ec=1
fi
# Remove each key line and the comment lines that follow it, up to the next key.
sed -e '/^  idTokenMarkerClaims:/,/^  scopes:/{/^  scopes:/!d;}' \
  -e '/^  accessTokenRequiredClaims:/,/^  authorizationAudience:/{/^  authorizationAudience:/!d;}' \
  "$TMP/chart-no-claim-lists/values.yaml" >"$TMP/values-no-claim-lists.yaml"
cp "$TMP/values-no-claim-lists.yaml" "$TMP/chart-no-claim-lists/values.yaml"
if grep -q '^  idTokenMarkerClaims:\|^  accessTokenRequiredClaims:' "$TMP/chart-no-claim-lists/values.yaml"; then
  echo "FAIL [R7 setup]: the chart copy still has a claim-list key in values.yaml"; ec=1
fi

check_required "R1 default"                   -                  absent  absent
check_required "R2 reuse-values-no-key"       -                  absent  absent  --set oidc.accessTokenRequiredClaims=null
check_required "R3 set"                       "$REQUIRED_SUFFIX" absent  present --set 'oidc.accessTokenRequiredClaims={jti}'
check_required "R4 both-lists"                "$BOTH_SUFFIX"     present present -f "$TMP/required-both.yaml"
check_required "R5 empty-list-set-json"       -                  absent  absent  --set-json 'oidc.accessTokenRequiredClaims=[]'
check_required_restart "R6 restart-scope"
REQUIRED_CHART="$TMP/chart-no-claim-lists" \
check_required "R7 reuse-values-nil-guard"    -                  absent  absent \
  --set oidc.idTokenMarkerClaims=null --set oidc.accessTokenRequiredClaims=null
REQUIRED_CHART="$TMP/chart-no-claim-lists" \
check_required "R8 nil-markers-with-required" "$REQUIRED_SUFFIX" absent  present \
  --set oidc.idTokenMarkerClaims=null --set 'oidc.accessTokenRequiredClaims={jti}'

if [ "$REQUIRED_ROWS" -lt "$REQUIRED_ROWS_WANT" ]; then
  echo "FAIL [required rows]: $REQUIRED_ROWS required row(s) ran, want $REQUIRED_ROWS_WANT"; ec=1
fi

# zones.iam.backend.dpop (SMA-700). Renders go to a file, as for check_boot. One row per property:
```

- [ ] **Step 2: Write the failing `refusals.sh` rows**

In `charts/paigasus/tests/refusals.sh`, Edit `old_string`:

```bash
# SMA-700 (spec § 4.11). zones.iam.backend.dpop copies the IamConfig::validate rules for the URL
```

`new_string`:

```bash
# SMA-731 (spec D5). oidc.accessTokenRequiredClaims goes through the same template as
# oidc.idTokenMarkerClaims (paigasus.validateClaimNameList), with its own path, example and
# reserved-name reason. One more rule: a name must not be in both lists. Each needle carries the
# key path and the index.
REQUIRED=oidc.accessTokenRequiredClaims
expect_fail "required not a list" "oidc.accessTokenRequiredClaims must be a list of claim names, for example [\"jti\"]" \
  --set "$REQUIRED=jti"
expect_fail "required item a number" "oidc.accessTokenRequiredClaims[0] must be a string" \
  --set "$REQUIRED={123}"
# `--set x={}` does not clear a list: helm 3.22.0 makes it [""].
expect_fail "required set to {}" "oidc.accessTokenRequiredClaims[0] is empty. IamConfig::validate refuses an empty name, and IAM does not boot. To remove the value, use [] in a values file or --set-json 'oidc.accessTokenRequiredClaims=[]'" \
  --set "$REQUIRED={}"
expect_fail "required item with a space" "oidc.accessTokenRequiredClaims[1] is \" nbf\"" \
  --set "$REQUIRED={jti, nbf}"
expect_fail "required item with a control character" "oidc.accessTokenRequiredClaims[0] is \"a\\x01b\"" \
  --set-string "$REQUIRED[0]=$(printf 'a\001b')"
expect_fail "required item with a quote" "oidc.accessTokenRequiredClaims[0] is \"a\\\"b\"" \
  --set-string "$REQUIRED[0]=a\"b"
expect_fail "required item not ASCII" "oidc.accessTokenRequiredClaims[0] is \"jti" \
  --set-string "$REQUIRED[0]=$(printf 'jti\303\251')"
REQUIRED_BS="$(mktemp)"
printf 'oidc:\n  accessTokenRequiredClaims: ['"'"'a\\b'"'"']\n' >"$REQUIRED_BS"
expect_fail "required item with a backslash" "oidc.accessTokenRequiredClaims[0] is \"a\\\\b\"" \
  -f "$REQUIRED_BS"
rm -f "$REQUIRED_BS"
for reserved in iss sub aud exp; do
  expect_fail "required reserved name $reserved" "oidc.accessTokenRequiredClaims[1] is \"$reserved\": every token that IAM accepts carries this claim, so the name has no effect" \
    --set "$REQUIRED={jti,$reserved}"
done
expect_fail "required duplicate" "oidc.accessTokenRequiredClaims[2] \"jti\" is already in oidc.accessTokenRequiredClaims[0]" \
  --set "$REQUIRED={jti,nbf,jti}"
expect_fail "required name also a marker" "oidc.accessTokenRequiredClaims[1] \"azp\" is also in oidc.idTokenMarkerClaims[1]" \
  --set "$MARKERS={at_hash,azp}" --set "$REQUIRED={jti,azp}"
expect_render "required Zitadel recipe" \
  --set "$REQUIRED={jti}"
expect_render "required and markers, Zitadel recipe" \
  --set "$MARKERS={at_hash,azp}" --set "$REQUIRED={jti}"
# Review Focus 2. Names compare exactly, as in IAM: JTI and jti are two names.
expect_render "required JTI and marker jti" \
  --set "$MARKERS={jti}" --set "$REQUIRED={JTI}"

# SMA-700 (spec § 4.11). zones.iam.backend.dpop copies the IamConfig::validate rules for the URL
```

- [ ] **Step 3: Run the rows and see them fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims
helm version --short
/bin/bash charts/paigasus/tests/env.sh --set ingress.host=console.example.test; echo "rc=$?"
/bin/bash charts/paigasus/tests/refusals.sh --set ingress.host=console.example.test; echo "rc=$?"
```

Expected (MEASURED during planning): `helm version` prints `v3.22.0+g144ca65`. `env.sh` rc=1 with
`FAIL [R7 setup]: values.yaml has no accessTokenRequiredClaims line to remove`,
`FAIL [R3 set]`, `FAIL [R4 both-lists]`, `FAIL [R8 nil-markers-with-required]` (the value has no
`access_token_required_claims`, and the required comment line renders 0 times) and
`FAIL [R6 restart-scope]: iam-backend: spec.template is equal; it must differ`; R1, R2, R5, R7
and all M rows are `ok`. `refusals.sh` rc=1 with `rendered, expected a refusal` for all 14
`required …` `expect_fail` rows; the three `required …` `expect_render` rows are `ok`.

- [ ] **Step 4: Add the value to `values.yaml`**

In `charts/paigasus/values.yaml`, Edit `old_string`:

```yaml
  authorizationAudience: ""   # NOT required. The audience parameter that both consoles send in
```

`new_string`:

```yaml
  accessTokenRequiredClaims: []   # NOT required. Claim names that the IdP puts into every
                                  # access token and into no ID token (SMA-731). IAM refuses a
                                  # token that does not carry one of them, or carries it with the
                                  # value null. Empty: no such check. Zitadel: ["jti"]. Use it with
                                  # oidc.idTokenMarkerClaims, and never name a claim in both lists
                                  # (the render fails). This check fails closed: if the IdP stops
                                  # putting the claim into its access token, IAM refuses every
                                  # token of the IdP. Each name is printable ASCII with no space,
                                  # " or \, and not iss, sub, aud or exp. A change restarts IAM. To
                                  # remove it, set [] in a values file or use
                                  # --set-json 'oidc.accessTokenRequiredClaims=[]', not {}. With
                                  # --reuse-values, deleting the key from a values file does not
                                  # remove the old list. See RUNBOOK-chart.md § 6.
  authorizationAudience: ""   # NOT required. The audience parameter that both consoles send in
```

- [ ] **Step 5: Make the marker templates shared, and add the required list and the overlap check**

In `charts/paigasus/templates/_iam-backend.tpl`, Edit `old_string`:

```
paigasus.validateIamBackend: the refusals for the two values, and for oidc.idTokenMarkerClaims
(SMA-703, paigasus.validateIdTokenMarkerClaims below) and zones.iam.backend.dpop (SMA-700,
paigasus.validateIamDpop below). paigasus.validate calls it.
```

`new_string`:

```
paigasus.validateIamBackend: the refusals for the two values, for oidc.idTokenMarkerClaims and
oidc.accessTokenRequiredClaims (SMA-703, SMA-731: paigasus.validateClaimNameList and
paigasus.validateClaimListOverlap below), and for zones.iam.backend.dpop (SMA-700,
paigasus.validateIamDpop below). paigasus.validate calls it.
```

Edit `old_string`:

```
{{- include "paigasus.validateIdTokenMarkerClaims" . -}}
```

`new_string`:

```
{{- include "paigasus.validateClaimNameList" (dict "root" . "key" "idTokenMarkerClaims" "path" "oidc.idTokenMarkerClaims" "example" "[\"at_hash\", \"azp\"]" "reserved" "every access token carries this claim, so IAM would refuse every token") -}}
{{- include "paigasus.validateClaimNameList" (dict "root" . "key" "accessTokenRequiredClaims" "path" "oidc.accessTokenRequiredClaims" "example" "[\"jti\"]" "reserved" "every token that IAM accepts carries this claim, so the name has no effect") -}}
{{- include "paigasus.validateClaimListOverlap" . -}}
```

Then replace the two SMA-703 helpers. Edit `old_string` (from the `paigasus.iamIdTokenMarkerClaims`
doc comment to the end of `paigasus.validateIdTokenMarkerClaims`, the current lines 109-168):

```
{{/*
paigasus.iamIdTokenMarkerClaims: the suffix ,id_token_marker_claims=[...] for the one issuer entry
of IAM_AUTHN__ISSUERS (SMA-703), or "" when oidc.idTokenMarkerClaims is empty, absent or nil. An
absent key comes from `helm upgrade --reuse-values` on a release made before the key; dig then
gives the default. Each name is quoted with %q, like the audience. figment reads the inline form
(the test issuers_env_in_the_chart_form_parses_id_token_marker_claims in
rs/crates/services/paigasus-iam/src/config.rs). paigasus.validateIdTokenMarkerClaims has already
refused every name that %q would escape.
*/}}
{{- define "paigasus.iamIdTokenMarkerClaims" -}}
{{- $names := dig "idTokenMarkerClaims" list .Values.oidc -}}
{{- if and (kindIs "slice" $names) $names -}}
{{- $quoted := list -}}
{{- range $names -}}
{{- $quoted = append $quoted (printf "%q" .) -}}
{{- end -}}
{{- printf ",id_token_marker_claims=[%s]" (join "," $quoted) -}}
{{- end -}}
{{- end -}}

{{/*
paigasus.validateIdTokenMarkerClaims: the refusals for oidc.idTokenMarkerClaims (SMA-703 D5). A
bad name stops IAM at boot (IamConfig::validate), and the IAM Deployment has one replica with
maxSurge 0, so the old pod stops before the new pod fails. So the chart copies the boot rules and
fails the render instead. The character rule is stricter than IAM: printable ASCII only, with no
space, no " and no \. Go's %q writes other characters as escapes (\t, \x01) that figment does not
read. The rule also refuses leading and trailing whitespace. A nil value counts as an empty list.
The nil case happens when a release from before SMA-703 has no key and the user passes
--set oidc.idTokenMarkerClaims=null. Then Helm keeps a nil value in the user values.
*/}}
{{- define "paigasus.validateIdTokenMarkerClaims" -}}
{{- $names := dig "idTokenMarkerClaims" list .Values.oidc -}}
{{- if kindIs "invalid" $names -}}
{{- $names = list -}}
{{- end -}}
{{- if not (kindIs "slice" $names) -}}
{{- fail "oidc.idTokenMarkerClaims must be a list of claim names, for example [\"at_hash\", \"azp\"]" -}}
{{- end -}}
{{- $seen := dict -}}
{{- range $i, $n := $names -}}
{{- if not (kindIs "string" $n) -}}
{{- fail (printf "oidc.idTokenMarkerClaims[%d] must be a string. Quote the name in a values file, or use --set-string" $i) -}}
{{- end -}}
{{- if not $n -}}
{{- fail (printf "oidc.idTokenMarkerClaims[%d] is empty. IamConfig::validate refuses an empty name, and IAM does not boot. To remove the value, use [] in a values file or --set-json 'oidc.idTokenMarkerClaims=[]', not --set oidc.idTokenMarkerClaims={}" $i) -}}
{{- end -}}
{{- if not (regexMatch `^[!#-\[\]-~]+$` $n) -}}
{{- fail (printf "oidc.idTokenMarkerClaims[%d] is %q: use printable ASCII only, with no space, no \" and no \\. IAM cannot read another character from IAM_AUTHN__ISSUERS" $i $n) -}}
{{- end -}}
{{- /* Keep this list equal to RESERVED_MARKER_CLAIMS in rs/crates/services/paigasus-iam/src/config.rs. */ -}}
{{- if has $n (list "iss" "sub" "aud" "exp") -}}
{{- fail (printf "oidc.idTokenMarkerClaims[%d] is %q: every access token carries this claim, so IAM would refuse every token. IamConfig::validate refuses it, and IAM does not boot" $i $n) -}}
{{- end -}}
{{- if hasKey $seen $n -}}
{{- fail (printf "oidc.idTokenMarkerClaims[%d] %q is already in oidc.idTokenMarkerClaims[%d]. IamConfig::validate refuses a duplicate, and IAM does not boot" $i $n (index $seen $n)) -}}
{{- end -}}
{{- $_ := set $seen $n $i -}}
{{- end -}}
{{- end -}}
```

`new_string`:

```
{{/*
paigasus.iamIssuerClaimList: the suffix ,<field>=[...] for the one issuer entry of
IAM_AUTHN__ISSUERS, or "" when the list is empty, absent or nil (SMA-703, SMA-731). The argument
is a dict:
  root   the root context
  key    the key under oidc, read with dig (idTokenMarkerClaims)
  field  the IssuerConfig field name (id_token_marker_claims)
An absent key comes from `helm upgrade --reuse-values` on a release made before the key; dig then
gives the default. Each name is quoted with %q, like the audience. figment reads the inline form
(the tests issuers_env_in_the_chart_form_parses_id_token_marker_claims and
issuers_env_in_the_chart_form_parses_access_token_required_claims in
rs/crates/services/paigasus-iam/src/config.rs). paigasus.validateClaimNameList has already
refused every name that %q would escape.
*/}}
{{- define "paigasus.iamIssuerClaimList" -}}
{{- $names := dig .key list .root.Values.oidc -}}
{{- if and (kindIs "slice" $names) $names -}}
{{- $quoted := list -}}
{{- range $names -}}
{{- $quoted = append $quoted (printf "%q" .) -}}
{{- end -}}
{{- printf ",%s=[%s]" .field (join "," $quoted) -}}
{{- end -}}
{{- end -}}

{{/*
paigasus.iamIdTokenMarkerClaims: the suffix for oidc.idTokenMarkerClaims (SMA-703). Takes the
ROOT context.
*/}}
{{- define "paigasus.iamIdTokenMarkerClaims" -}}
{{- include "paigasus.iamIssuerClaimList" (dict "root" . "key" "idTokenMarkerClaims" "field" "id_token_marker_claims") -}}
{{- end -}}

{{/*
paigasus.iamAccessTokenRequiredClaims: the suffix for oidc.accessTokenRequiredClaims (SMA-731).
Takes the ROOT context.
*/}}
{{- define "paigasus.iamAccessTokenRequiredClaims" -}}
{{- include "paigasus.iamIssuerClaimList" (dict "root" . "key" "accessTokenRequiredClaims" "field" "access_token_required_claims") -}}
{{- end -}}

{{/*
paigasus.validateClaimNameList: the refusals for one claim-name list under oidc (SMA-703 D5,
SMA-731 D5). paigasus.validateIamBackend calls it once for oidc.idTokenMarkerClaims and once for
oidc.accessTokenRequiredClaims. The argument is a dict:
  root      the root context
  key       the key under oidc, read with dig (idTokenMarkerClaims)
  path      the value path that each message names (oidc.idTokenMarkerClaims)
  example   the example list of the "not a list" message (["at_hash", "azp"])
  reserved  the reason of the reserved-name message
A bad name stops IAM at boot (IamConfig::validate), and the IAM Deployment has one replica with
maxSurge 0, so the old pod stops before the new pod fails. So the chart copies the boot rules and
fails the render instead. The character rule is stricter than IAM: printable ASCII only, with no
space, no " and no \. Go's %q writes other characters as escapes (\t, \x01) that figment does not
read. The rule also refuses leading and trailing whitespace. A nil value counts as an empty list.
The nil case happens when a release from before the key has no key and the user passes
--set oidc.<key>=null. Then Helm keeps a nil value in the user values.
*/}}
{{- define "paigasus.validateClaimNameList" -}}
{{- $path := .path -}}
{{- $reserved := .reserved -}}
{{- $names := dig .key list .root.Values.oidc -}}
{{- if kindIs "invalid" $names -}}
{{- $names = list -}}
{{- end -}}
{{- if not (kindIs "slice" $names) -}}
{{- fail (printf "%s must be a list of claim names, for example %s" $path .example) -}}
{{- end -}}
{{- $seen := dict -}}
{{- range $i, $n := $names -}}
{{- if not (kindIs "string" $n) -}}
{{- fail (printf "%s[%d] must be a string. Quote the name in a values file, or use --set-string" $path $i) -}}
{{- end -}}
{{- if not $n -}}
{{- fail (printf "%s[%d] is empty. IamConfig::validate refuses an empty name, and IAM does not boot. To remove the value, use [] in a values file or --set-json '%s=[]', not --set %s={}" $path $i $path $path) -}}
{{- end -}}
{{- if not (regexMatch `^[!#-\[\]-~]+$` $n) -}}
{{- fail (printf "%s[%d] is %q: use printable ASCII only, with no space, no \" and no \\. IAM cannot read another character from IAM_AUTHN__ISSUERS" $path $i $n) -}}
{{- end -}}
{{- /* Keep this list equal to RESERVED_CLAIM_NAMES in rs/crates/services/paigasus-iam/src/config.rs. */ -}}
{{- if has $n (list "iss" "sub" "aud" "exp") -}}
{{- fail (printf "%s[%d] is %q: %s. IamConfig::validate refuses it, and IAM does not boot" $path $i $n $reserved) -}}
{{- end -}}
{{- if hasKey $seen $n -}}
{{- fail (printf "%s[%d] %q is already in %s[%d]. IamConfig::validate refuses a duplicate, and IAM does not boot" $path $i $n $path (index $seen $n)) -}}
{{- end -}}
{{- $_ := set $seen $n $i -}}
{{- end -}}
{{- end -}}

{{/*
paigasus.validateClaimListOverlap: a name in both oidc.idTokenMarkerClaims and
oidc.accessTokenRequiredClaims (SMA-731 D2, D5). IAM would then refuse every token of the issuer:
a token with the claim is refused as an ID token, and a token without it as missing it.
IamConfig::validate refuses it, and IAM does not boot. It runs after both lists passed
paigasus.validateClaimNameList, so every item is a string. A nil list counts as empty. Names
compare exactly (case-sensitive), as in IAM.
*/}}
{{- define "paigasus.validateClaimListOverlap" -}}
{{- $markers := dig "idTokenMarkerClaims" list .Values.oidc -}}
{{- if kindIs "invalid" $markers -}}
{{- $markers = list -}}
{{- end -}}
{{- $required := dig "accessTokenRequiredClaims" list .Values.oidc -}}
{{- if kindIs "invalid" $required -}}
{{- $required = list -}}
{{- end -}}
{{- $markerAt := dict -}}
{{- range $i, $n := $markers -}}
{{- $_ := set $markerAt $n $i -}}
{{- end -}}
{{- range $i, $n := $required -}}
{{- if hasKey $markerAt $n -}}
{{- fail (printf "oidc.accessTokenRequiredClaims[%d] %q is also in oidc.idTokenMarkerClaims[%d]. IAM would refuse every token of this issuer: a token with the claim is refused as an ID token, and a token without it as missing it. IamConfig::validate refuses it, and IAM does not boot" $i $n (index $markerAt $n)) -}}
{{- end -}}
{{- end -}}
{{- end -}}
```

The SMA-703 messages do not change: the path, the example `["at_hash", "azp"]` and the reserved
reason come from the dict, so the existing `markers …` rows of `refusals.sh` still match.

- [ ] **Step 6: Render the suffix in `backend-deployment.yaml`**

In `charts/paigasus/templates/backend-deployment.yaml`, Edit `old_string`:

```
{{- if include "paigasus.iamIdTokenMarkerClaims" $root }}
            # oidc.idTokenMarkerClaims is set: IAM refuses a token that carries one of these claims.
{{- end }}
            - name: IAM_AUTHN__ISSUERS
              value: {{ printf "[{issuer=%q,audiences=[%q]%s}]" $root.Values.oidc.issuer (include "paigasus.iamAudience" $root) (include "paigasus.iamIdTokenMarkerClaims" $root) | quote }}
```

`new_string`:

```
{{- if include "paigasus.iamIdTokenMarkerClaims" $root }}
            # oidc.idTokenMarkerClaims is set: IAM refuses a token that carries one of these claims.
{{- end }}
{{- /*
  oidc.accessTokenRequiredClaims (SMA-731). The same pattern: "" for the default [], and the
  comment line only inside the `if`, so the default render stays byte-identical.
*/}}
{{- if include "paigasus.iamAccessTokenRequiredClaims" $root }}
            # oidc.accessTokenRequiredClaims is set: IAM refuses a token that lacks one of these claims.
{{- end }}
            - name: IAM_AUTHN__ISSUERS
              value: {{ printf "[{issuer=%q,audiences=[%q]%s%s}]" $root.Values.oidc.issuer (include "paigasus.iamAudience" $root) (include "paigasus.iamIdTokenMarkerClaims" $root) (include "paigasus.iamAccessTokenRequiredClaims" $root) | quote }}
```

- [ ] **Step 7: Run every chart script and see them pass, goldens unchanged**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims
for s in render refusals env maps ingress names ca-bundle; do
  /bin/bash "charts/paigasus/tests/$s.sh" --set ingress.host=console.example.test >"${TMPDIR:-/tmp}/sma731-$s.txt" 2>&1
  echo "$s rc=$?"; grep -E "FAIL|OK ==" "${TMPDIR:-/tmp}/sma731-$s.txt"
done
git diff --stat -- charts/paigasus/tests/golden
```

Expected (MEASURED during planning): every script rc=0 and prints its `== … OK ==` line, with no
`FAIL`; `env.sh` shows `ok [M1 default]` to `ok [M8 reuse-values-nil-guard]` and
`ok [R1 default]` to `ok [R8 nil-markers-with-required]`; `refusals.sh` shows the old `markers …`
rows and the 17 new `required …` rows `ok`. `git diff --stat -- charts/paigasus/tests/golden`
prints nothing.

- [ ] **Step 8: Run the helm-render gate**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims
moon run repo:helm-render
```

Expected: PASS. `ci/helm-render/run.sh` runs under bash 3.2 and bash 5 (its own header). If it
stops at about 0% CPU, the host is in the 512-byte-pipe state (root `CLAUDE.md`, "This
development Mac only"). Then run `/bin/bash ci/helm-render/run.sh` and read its verdict. Do not
edit `ci/helm-render/`.

- [ ] **Step 9: Document the value in the chart README**

In `charts/paigasus/README.md`, Edit `old_string`:

```
  are in `paigasus.validateIdTokenMarkerClaims` in `templates/_iam-backend.tpl`. They are not in
```

`new_string`:

```
  are in `paigasus.validateClaimNameList` in `templates/_iam-backend.tpl`. They are not in
```

Edit `old_string`:

```
`docs/ops/RUNBOOK-chart.md` § 6 for the IdP setup.

## DPoP on the gateway path (`zones.iam.backend.dpop`)
```

`new_string`:

```
`docs/ops/RUNBOOK-chart.md` § 6 for the IdP setup.

## The required access-token claims (`oidc.accessTokenRequiredClaims`)

IAM can refuse a token that does not carry a claim which the IdP puts into every access token
and into no ID token (SMA-731). It is the fail-closed partner of `oidc.idTokenMarkerClaims`. The
chart renders the list into the one issuer entry of `IAM_AUTHN__ISSUERS`, after the marker list.

- **Empty or absent (the default).** The chart adds nothing. The render is byte-identical to a
  chart without the value. A nil value from `--reuse-values` counts as an empty list.
- **Set.** The entry gets `,access_token_required_claims=["jti"]` after `audiences` and after the
  marker list. Each name is quoted with `%q`. One more YAML comment line renders above
  `IAM_AUTHN__ISSUERS`:
  `# oidc.accessTokenRequiredClaims is set: IAM refuses a token that lacks one of these claims.`
- **The render fails** for the same values as for `oidc.idTokenMarkerClaims`. The two lists use
  one template, `paigasus.validateClaimNameList` in `templates/_iam-backend.tpl`. A `dict`
  argument gives each list its own path, example and reserved-name reason in the messages. The
  render also fails when a name is in both lists (`paigasus.validateClaimListOverlap`), because
  IAM refuses that at boot: it would refuse every token of the issuer. Names compare exactly, so
  `JTI` and `jti` are two names.
- **To remove the value,** set `[]` in a values file, or use
  `--set-json 'oidc.accessTokenRequiredClaims=[]'`. Do not use
  `--set oidc.accessTokenRequiredClaims={}`: Helm makes it a list with one empty name, and the
  render fails.
- A change of the value restarts the IAM pod and no console pod.

`tests/env.sh` holds the rows `R1 default` to `R8 nil-markers-with-required`, with a row counter.
`tests/refusals.sh` holds one row for each refusal and three valid renders. See
`docs/ops/RUNBOOK-chart.md` § 6 for the IdP setup.

## DPoP on the gateway path (`zones.iam.backend.dpop`)
```

Also fix the two template names in the `env.sh` comments. Edit `old_string`:

```bash
#                   paigasus.validateIdTokenMarkerClaims. The value does not change (spec T20).
```

`new_string`:

```bash
#                   paigasus.validateClaimNameList. The value does not change (spec T20).
```

Edit `old_string`:

```bash
#                   in paigasus.validateIdTokenMarkerClaims. The render must succeed with the
```

`new_string`:

```bash
#                   in paigasus.validateClaimNameList. The render must succeed with the
```

Check that the old name is gone:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims
grep -rn 'validateIdTokenMarkerClaims\|RESERVED_MARKER_CLAIMS' charts rs/crates docs/ops || echo "none"
```

Expected: `none`.

- [ ] **Step 10: Run `env.sh` once more, then commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims
/bin/bash charts/paigasus/tests/env.sh --set ingress.host=console.example.test | tail -1
git add charts/paigasus/values.yaml charts/paigasus/templates/_iam-backend.tpl \
  charts/paigasus/templates/backend-deployment.yaml charts/paigasus/README.md \
  charts/paigasus/tests/env.sh charts/paigasus/tests/refusals.sh
git commit -m "feat(repo): chart value oidc.accessTokenRequiredClaims for the IAM issuer (SMA-731)" \
  -m "Both claim lists now use one parameterized validation template, so the SMA-703 messages
stay and the new list gets the same rules. A new check refuses a name in both lists. The chart
renders the required list after the marker list only when it is not empty, so the default
render and the golden files do not change. env.sh gets rows R1 to R8, and refusals.sh one row
for each refusal." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git log --oneline -1
```

Expected: the `tail -1` line is `== chart env OK ==`.

---

### Task 8: Runbook § 6, `iam.toml.example` and the CHANGELOG

**Files:**
- Modify: `docs/ops/RUNBOOK-chart.md` (§ 1 values table `:38`, § 5 restart table `:141`, § 6 paragraph after the SMA-703 list `:229`, per-IdP header `:421-422`, Keycloak bullet `:424-428`, Dex bullet `:457-462`, Zitadel bullet `:497-498` and `:506-510`)
- Modify: `rs/crates/services/paigasus-iam/iam.toml.example:73-77`
- Modify: `rs/crates/services/paigasus-iam/CHANGELOG.md` (`## [Unreleased]` / `### Added`, after the SMA-700 chart line `:30-31`)

**Interfaces:**
- Consumes: the value name (Task 7), the log message, the marker `missing claim <name>` and the boot line (Task 3), the `env.sh` row name `R6` (Task 7), the Keycloak branch (Task 1).
- Produces: documentation only.

- [ ] **Step 1: Write the failing text check**

Save nothing to the repo. Run this check; it fails before the edits:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims
python3 -c '
import sys
checks = {
    "docs/ops/RUNBOOK-chart.md": [
        "| `oidc.accessTokenRequiredClaims` | no |",
        "| `oidc.accessTokenRequiredClaims` | the IAM pod, not the consoles |",
        "**IAM refuses a token that does not carry a claim that you configure (SMA-731).**",
        "refused a bearer token: it does not carry a claim that the issuer configuration requires",
        "`missing claim <name>`",
        "What the console shows a user in this",
        "Also set `oidc.accessTokenRequiredClaims: [\"jti\"]`.",
        "the JWT-profile grant",
        "--set-json \x27oidc.accessTokenRequiredClaims=[]\x27",
    ],
    "rs/crates/services/paigasus-iam/iam.toml.example": [
        "# access_token_required_claims = [\"jti\"]",
    ],
    "rs/crates/services/paigasus-iam/CHANGELOG.md": [
        "`access_token_required_claims`",
        "(SMA-731)",
    ],
}
missing = [(path, n) for path, needles in checks.items() for n in needles if n not in open(path).read()]
print("missing:", missing) if missing else print("OK")
sys.exit(1 if missing else 0)'; echo "rc=$?"
```

Expected: rc=1, with all twelve strings listed as missing.

- [ ] **Step 2: Add the values-table row (§ 1) and the restart-table row (§ 5)**

In `docs/ops/RUNBOOK-chart.md`, Edit `old_string`:

```
| `oidc.idTokenMarkerClaims` | no | Claim names that the IdP puts into its ID token and never into its access token. IAM refuses a token that carries one of them. Default `[]`: no such check. Zitadel: `["at_hash", "azp"]` (§ 6) |
```

`new_string`:

```
| `oidc.idTokenMarkerClaims` | no | Claim names that the IdP puts into its ID token and never into its access token. IAM refuses a token that carries one of them. Default `[]`: no such check. Zitadel: `["at_hash", "azp"]` (§ 6) |
| `oidc.accessTokenRequiredClaims` | no | Claim names that the IdP puts into every access token and into no ID token. IAM refuses a token that does not carry one of them. Default `[]`: no such check. Zitadel: `["jti"]`. The check fails closed (§ 6) |
```

Edit `old_string`:

```
| `oidc.idTokenMarkerClaims` | the IAM pod, not the consoles | it changes `IAM_AUTHN__ISSUERS` in the IAM pod template (`tests/env.sh` row M6). IAM is not available during the restart, as for `oidc.audience` |
```

`new_string`:

```
| `oidc.idTokenMarkerClaims` | the IAM pod, not the consoles | it changes `IAM_AUTHN__ISSUERS` in the IAM pod template (`tests/env.sh` row M6). IAM is not available during the restart, as for `oidc.audience` |
| `oidc.accessTokenRequiredClaims` | the IAM pod, not the consoles | it changes `IAM_AUTHN__ISSUERS` in the IAM pod template (`tests/env.sh` row R6). IAM is not available during the restart, as for `oidc.audience` |
```

- [ ] **Step 3: Add the SMA-731 paragraph to § 6**

Edit `old_string`:

```
  `--set oidc.idTokenMarkerClaims={}`: Helm makes it a list with one empty name, and the chart
  refuses it.

**IAM refuses a sender-constrained token (SMA-690).** IAM refuses an access token that has one
```

`new_string`:

```
  `--set oidc.idTokenMarkerClaims={}`: Helm makes it a list with one empty name, and the chart
  refuses it.

**IAM refuses a token that does not carry a claim that you configure (SMA-731).** Set
`oidc.accessTokenRequiredClaims` to a list of claim names. IAM then refuses a token that does
not carry one of these claims, or carries it with the value `null`. The default is `[]`, and IAM
then does no such check. The marker claims above refuse a token that has an ID-token claim. This
setting refuses a token that does not have an access-token claim. Use both when your IdP has
such claims.

- Use a name only when the IdP puts it into every access token and into no ID token. Before you
  set the value, decode one access token for each grant type in use. Each access token must have
  the claim. Decode one ID token. It must not have the claim.
- The check fails closed. If the IdP stops putting the claim into its access token, for example
  after an IdP upgrade, IAM refuses every request of that issuer with a `401` and the reason
  `invalid-token`. The IAM log then has this line at `info` level:
  "refused a bearer token: it does not carry a claim that the issuer configuration requires".
  The line has the issuer and the marker `missing claim <name>`, for example
  `missing claim jti`. It does not show a claim value. What the console shows a user in this
  case is not measured.
- The rate limit above applies. It is shared with the other refusals of an ID token for the
  issuer. So one line can stand for many refusals, and a line about a marker claim can hide a
  line about a missing claim for 10 seconds. If the IAM log level hides `info`, the line does not
  show.
- To recover from a fail-closed refusal, remove the value (see the last item). Then decode the
  new tokens before you set a new name.
- At start, IAM writes one `info` line with the issuer and the configured names. When the log
  level includes `info` and the line is not in the IAM log, IAM does not use the setting.
- The chart refuses the same names as for `oidc.idTokenMarkerClaims`: an empty name, a name with
  a character outside printable ASCII, a space, `"` or `\`, the names `iss`, `sub`, `aud` and
  `exp`, and a name that occurs two times. It also refuses a name that is in both lists, because
  IAM would then refuse every token of the issuer. Names compare exactly: `jti` and `JTI` are
  two names.
- A change of the value restarts IAM (§ 5).
- To remove the setting, set it to `[]` in a values file, or use
  `--set-json 'oidc.accessTokenRequiredClaims=[]'`. With `helm upgrade --reuse-values`, deleting
  the key from a values file does not remove the old list. Do not use
  `--set oidc.accessTokenRequiredClaims={}`: Helm makes it a list with one empty name, and the
  chart refuses it.

**IAM refuses a sender-constrained token (SMA-690).** IAM refuses an access token that has one
```

- [ ] **Step 4: The per-IdP header, the Keycloak bullet and the Dex bullet**

Edit `old_string`:

```
**Per IdP. Not measured.** These lines state what each IdP offers. This chart did not measure
them. The Zitadel item is measured, except where it says otherwise.
```

`new_string`:

```
**Per IdP. Not measured.** These lines state what each IdP offers. This chart did not measure
them. The Zitadel item is measured, except where it says otherwise. The `jti` sentence of the
Keycloak item is measured.
```

Keycloak, branch B (the planning result). Edit `old_string`:

```
  example 2 below shows the mapper. IAM also refuses a Keycloak ID token by its `typ` claim
  (SMA-686).
```

`new_string`:

```
  example 2 below shows the mapper. IAM also refuses a Keycloak ID token by its `typ` claim
  (SMA-686). `oidc.accessTokenRequiredClaims: ["jti"]` does not help for Keycloak. Measured on
  Keycloak 26.4 (SMA-731): the ID token and the access token both have `jti`, for the password
  grant and the refresh grant. So the `typ` check stays the Keycloak defence.
```

Keycloak, branch A (only if Task 1 showed the split). Use this `new_string` instead:

```
  example 2 below shows the mapper. IAM also refuses a Keycloak ID token by its `typ` claim
  (SMA-686). As a second defence, set `oidc.accessTokenRequiredClaims: ["jti"]`. Measured on
  Keycloak 26.4 (SMA-731) with the password grant, the refresh grant and a DPoP-bound token:
  every access token has `jti`, and no ID token has it.
```

Edit `old_string`:

```
  - So SMA-686 residual R1 stays open for Dex (SMA-686 § 2).
```

`new_string`:

```
  - So SMA-686 residual R1 stays open for Dex (SMA-686 § 2).
  - `oidc.accessTokenRequiredClaims` is not measured for Dex.
```

- [ ] **Step 5: The Zitadel bullet**

Edit `old_string`:

```
  - Set `oidc.idTokenMarkerClaims: ["at_hash", "azp"]`. Every measured Zitadel ID token has both
    claims. No measured Zitadel access token has one of them.
```

`new_string`:

```
  - Set `oidc.idTokenMarkerClaims: ["at_hash", "azp"]`. Every measured Zitadel ID token has both
    claims. No measured Zitadel access token has one of them.
  - Also set `oidc.accessTokenRequiredClaims: ["jti"]`. Measured with Login v1: code flow, refresh
    grant, client-credentials grant (SMA-731). Every measured Zitadel access token has `jti`, and
    no measured Zitadel ID token has it. The two settings work together. The marker claims refuse
    a token that has an ID-token claim. The required claim refuses a token that does not have an
    access-token claim. If a future Zitadel version drops `at_hash` and `azp` from its ID token,
    the required claim still refuses the ID token. If it drops `jti` from its access token, every
    login fails, and the IAM log shows `missing claim jti`. Then remove the value, and check the
    new token shapes before you set a new name.
```

Edit `old_string`:

```
  - Before the switch: decode one access token for each grant type in use (authorization code,
    refresh token, client credentials). No access token can have `at_hash` or `azp`. Decode one
    ID token. It must have both claims. This check stays required. The paigasus console is
```

`new_string`:

```
  - Before the switch: decode one access token for each grant type in use (authorization code,
    refresh token, client credentials, and the JWT-profile grant). The JWT-profile grant is a
    usual Zitadel machine flow, and it is not measured. No access token can have `at_hash` or
    `azp`, and each access token must have `jti`. Decode one ID token. It must have `at_hash` and
    `azp`, and it must not have `jti`. This check stays required. The paigasus console is
```

- [ ] **Step 6: `iam.toml.example`**

In `rs/crates/services/paigasus-iam/iam.toml.example`, Edit `old_string`:

```
# id_token_marker_claims = ["at_hash", "azp"]   # default: none. Zitadel recipe shown.
#
# id_token_marker_claims lists claim names that the IdP puts into its ID token and never into its
# access token. IAM refuses a verified token that carries one of them with a value other than null.
```

`new_string`:

```
# id_token_marker_claims = ["at_hash", "azp"]   # default: none. Zitadel recipe shown.
# access_token_required_claims = ["jti"]        # default: none. Zitadel recipe shown.
#
# id_token_marker_claims lists claim names that the IdP puts into its ID token and never into its
# access token. IAM refuses a verified token that carries one of them with a value other than null.
#
# access_token_required_claims lists claim names that the IdP puts into every access token and
# into no ID token. IAM refuses a verified token that does not carry one of them, or carries it
# with the value null. This check fails closed: if the IdP stops putting the claim into its access
# token, IAM refuses every token of the issuer. A name must not be in both lists.
```

- [ ] **Step 7: The CHANGELOG**

In `rs/crates/services/paigasus-iam/CHANGELOG.md`, Edit `old_string`:

```
- The Helm chart has two new values: `zones.iam.backend.dpop.enabled` and
  `zones.iam.backend.dpop.forwardedBaseUrls`. The default render does not change (SMA-700).
```

`new_string`:

```
- The Helm chart has two new values: `zones.iam.backend.dpop.enabled` and
  `zones.iam.backend.dpop.forwardedBaseUrls`. The default render does not change (SMA-700).
- Each `[[authn.issuers]]` entry has a new setting, `access_token_required_claims`. It is a list
  of claim names that the IdP puts into every access token and into no ID token. For Zitadel, use
  `["jti"]`. The default is an empty list, and then IAM adds no new check. IAM refuses the same
  names as for `id_token_marker_claims`, and a name that is in both lists. IAM does not boot with
  such a list (SMA-731).
- IAM refuses a verified token that does not carry a configured claim, or carries it with the
  value `null`. The refusal is `NotAnAccessToken`. IAM logs it at `info` with the issuer, the
  marker `missing claim <name>` and its own message, "refused a bearer token: it does not carry a
  claim that the issuer configuration requires". The line does not show a claim value. The check
  fails closed (SMA-731).
- An issuer with only `access_token_required_claims` also uses the strict decode of SMA-703. IAM
  then refuses a token with a repeated top-level member as `Malformed` (SMA-731).
- At boot, IAM writes one `info` line for each issuer that has configured required claims. The
  line names the issuer and the claim names (SMA-731).
- The Helm chart has a new value, `oidc.accessTokenRequiredClaims`. It renders the list into
  `IAM_AUTHN__ISSUERS`. The default is `[]`, and the render does not change. A change of the value
  restarts the IAM pod (SMA-731).
```

- [ ] **Step 8: Run the text check and see it pass**

Run the command of Step 1 again.

Expected: `OK`, rc=0.

- [ ] **Step 9: Commit (two commits, one for each scope)**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims
git add docs/ops/RUNBOOK-chart.md
git commit -m "docs(repo): runbook gives the required claim jti and its fail-closed failure (SMA-731)" \
  -m "Section 6 gets the opt-in oidc.accessTokenRequiredClaims, what an operator sees when the IdP
drops the claim, the Zitadel recipe jti next to the marker claims, the JWT-profile grant in the
decode list, and the measured Keycloak result. Sections 1 and 5 get the value and its IAM
restart." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add rs/crates/services/paigasus-iam/iam.toml.example rs/crates/services/paigasus-iam/CHANGELOG.md
git commit -m "docs(rs): document access_token_required_claims in iam.toml.example and the changelog (SMA-731)" \
  -m "The example gets the commented key with the Zitadel recipe. The changelog gets the new
setting, the refusal and its log line, the strict decode, the boot line and the chart value." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git log --oneline -2
```

---

### Task 9: Final verification

**Files:** none changed.

**Interfaces:**
- Consumes: Tasks 1-8.
- Produces: the verdicts. No commit.

- [ ] **Step 1: Rust, fmt, clippy and the unit suites**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims/rs
cargo fmt --check -p paigasus-iam -p paigasus-iam-core
cargo clippy -p paigasus-iam -p paigasus-iam-core --all-targets --locked -- -D warnings
cargo nextest run -p paigasus-iam --lib --locked
cargo nextest run -p paigasus-iam-core --locked
```

Expected: fmt prints nothing; clippy exits 0; all lib tests PASS (974 or more in
`paigasus-iam`, 136 or more in `paigasus-iam-core`).

- [ ] **Step 2: The Docker suites that this change touches**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims/rs
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam \
  --test zitadel_e2e --test keycloak_e2e --test authn_private_ca --test docker_preflight
```

Expected: all PASS. `docker_preflight` proves that Docker was reachable. If a Docker suite
fails, follow root `CLAUDE.md` "Diagnosing an unattributed `moon ci` failure" Step 0 before a
re-run.

- [ ] **Step 3: The chart scripts and the helm-render gate**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims
for s in render refusals env maps ingress names ca-bundle; do
  /bin/bash "charts/paigasus/tests/$s.sh" --set ingress.host=console.example.test >"${TMPDIR:-/tmp}/sma731-$s.txt" 2>&1
  echo "$s rc=$?"
done
git diff --stat main...HEAD -- charts/paigasus/tests/golden
moon run repo:helm-render
```

Expected: every script rc=0; the golden diff prints nothing; `repo:helm-render` PASSES (it runs
under either bash; on a 512-byte-pipe host use `/bin/bash ci/helm-render/run.sh`).

- [ ] **Step 4: The Moon tasks of the touched projects**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims
moon run paigasus-iam-rs:lint paigasus-iam-rs:fmt paigasus-iam-core-rs:lint paigasus-iam-core-rs:test
PAIGASUS_REQUIRE_DOCKER=1 moon run paigasus-iam-rs:test
```

Expected: all PASS. `paigasus-iam-rs:test` runs every IAM Docker suite and takes several minutes.

- [ ] **Step 5: The full gate graph before the push (root `CLAUDE.md`)**

The coordinator runs the full `moon ci` command between the `ci-targets` markers of the root
`CLAUDE.md`, with `--base origin/main --include-relations`. On this Mac no single bash runs every
gate:
- `repo:affected-smoke` needs system `/bin/bash` 3.2 (run the graph with a bash-only shim
  directory that points at `/bin/bash`, see memory "affected-smoke hang").
- `repo:ruff-ci`, `repo:next-public-free` and `repo:publish-metadata` need bash 4+. Re-run them
  with `/opt/homebrew/bin/bash ci/<gate>/run.sh` and read that verdict.
- `repo:actionlint` needs bash 5 and a healthy pipe. Read its pipe preflight line. On a
  512-byte-pipe host there is no local verdict; it exits rc 2 with a `small` message.
- `repo:helm-render` runs under either bash.

- [ ] **Step 6: Branch state**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-731-required-claims
git log --oneline main..HEAD
git status --short
```

Expected: the spec commits (and the plan commit, if the coordinator made one), then the task
commits (Tasks 1, 2, 3, 4, 5 only for branch A, 7, and two for Task 8); `git status --short`
shows only this plan file if it is not committed yet.

## Spec coverage

| Spec item | Task |
|---|---|
| § 3 F4 (Keycloak result) | Task 1 Step 5 |
| D1 field, empty default, name | Task 2 Steps 3, 6 |
| D2 shared validator, reserved/empty/whitespace/duplicate, overlap, `RESERVED_CLAIM_NAMES` | Task 2 Steps 4-5 |
| D2 boot `info` line in `OidcAuthenticator::new` | Task 3 Step 7 (T14) |
| D3 step 6c order, presence rule, first missing name, decode choice, `ClaimRules`, both schemes | Task 3 Steps 6, 9, 10 |
| D4 `MissingClaim`, marker text, own message, same defect and rate-limit key, doc comments | Task 3 Steps 5, 6, 8, 11 |
| D5 chart value after `oidc.scopes`, `%q`, byte-identical default, `dig`/nil, render refusals, shared template with dict, overlap check, restart | Task 7 |
| D6 runbook § 1, § 5, § 6, Zitadel, Keycloak, Dex, JWT-profile grant; `iam.toml.example`; CHANGELOG | Task 8 (README in Task 7) |
| D7 no Notion ADR | no task (default kept) |
| D8 Keycloak measurement, temporary print, no claim that the ID-token refusal proves 6c | Tasks 1, 5 |
| D9 acceptance mapping | Tasks 3, 4, 8 |
| T1 `required_claim_refuses_zitadel_id_tokens` | Task 3 |
| T2 `required_claim_accepts_zitadel_access_tokens` | Task 3 |
| T3 `null_required_claim_is_missing_and_any_other_value_is_present` | Task 3 |
| T4 `the_first_missing_required_claim_is_logged` | Task 3 |
| T5 `required_claim_names_are_case_sensitive` | Task 3 |
| T6 `empty_claim_lists_accept_the_zitadel_id_token` | Task 3 |
| T7 `decode_defects_come_before_the_required_claim_check` | Task 3 |
| T8 `keycloak_typ_marker_runs_before_the_required_claims` | Task 3 |
| T9 `marker_claim_runs_before_the_required_claim` | Task 3 |
| T10 `required_claim_runs_before_the_sender_constraint_check` | Task 3 |
| T11 `duplicate_required_claim_member_is_malformed` | Task 3 |
| T12 `required_claims_are_per_issuer` | Task 3 |
| T13 `missing_claim_refusal_logs_its_own_message_issuer_and_name_only` | Task 3 |
| T14 `boot_line_names_the_issuer_and_the_required_claims` | Task 3 |
| T14a `dpop_scheme_refuses_a_bound_token_without_the_required_claim` | Task 3 |
| T14b `claim_rules_need_a_strict_decode_when_either_list_is_set`, `claim_rules_refusal_checks_markers_first_then_the_required_names` | Task 3 |
| T15 `validate_refuses_bad_access_token_required_claims`, `validate_refuses_a_name_in_both_claim_lists` | Task 2 |
| T16 `validate_refuses_bad_id_token_marker_claims` unchanged | Task 2 Step 7 |
| T17 `issuers_env_in_the_chart_form_parses_access_token_required_claims` | Task 2 |
| T18 F1 shape assertions in `zitadel_e2e.rs` | Task 4 |
| T19 required only, ID tokens refused, access tokens pass, 401 `invalid-token` | Task 4 |
| T19a full recipe | Task 4 |
| T20 Keycloak `jti` presence (and, for branch A, the IAM run) | Tasks 1, 5 |
| T21 SMA-703 assertions unchanged | Task 4 (`zitadel_config` delegates) |
| T22 `env.sh` R1-R8 | Task 7 |
| T23 `refusals.sh` rows | Task 7 |
| § 5 mutation proof (T1, T4, T14a, T19 fail) | Task 6 |
| Existing `IssuerConfig` literals | Task 2 Step 6 |
| § 6 typo in a raw `iam.toml` key | Task 2 (`a_misspelled_required_claims_key_is_ignored`) |
