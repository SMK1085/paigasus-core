# SMA-703 Zitadel ID-Token Marker Claims Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An operator can make IAM refuse a Zitadel ID token that a client sends as a bearer
token. The operator names, for each issuer, claims that only the IdP's ID token carries. The
default (an empty list) does not change the behaviour for any IdP.

**Architecture:** `IssuerConfig` gets `id_token_marker_claims: Vec<String>` (default empty), with
boot rules in `IamConfig::validate`. When the list of an issuer is not empty, the validator decodes
the verified payload as a `StrictPayload` (a map that refuses a repeated top-level member, plus the
usual `WireClaims`) and refuses a token that carries a configured name with a non-`null` value,
after the SMA-686 markers. The chart renders the list into `IAM_AUTHN__ISSUERS` from
`oidc.idTokenMarkerClaims` and copies the boot rules, and the runbook § 6 states Zitadel's measured
status.

**Tech Stack:** Rust 1.95 (edition 2024), `jsonwebtoken` 11.1.0, `serde` 1.0.228, `serde_json`
1.0.151, `figment` 0.10.19, `tracing`, `paigasus_logging::test_support`, `cargo nextest`; Helm
3.22.0 (Sprig templates), bash 3.2 and 5 chart scripts with inline `python3`.

**Spec:** `docs/superpowers/specs/2026-10-02-sma-703-zitadel-id-token-marker-claims-design.md`
(measurements: `docs/superpowers/specs/2026-10-02-sma-703-zitadel-measurements.md`).

## Global Constraints

- Work only in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-703-zitadel`, on branch `feature/sma-703-zitadel-id-token-check`.
- Put the proto shims first in every shell: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` and `export PROTO_REPORTER=text`.
- Every new source file starts with an SPDX header (`// SPDX-License-Identifier: Apache-2.0`, `#` for shell). This plan adds no new source file.
- Rust crates stay on edition 2024 and rust-version 1.95. Do not change any `Cargo.toml` or `rs/Cargo.lock`.
- Rust checks, run from `rs/`: `cargo fmt --check -p paigasus-iam`, `cargo clippy -p paigasus-iam --all-targets --locked -- -D warnings`, `cargo nextest run -p paigasus-iam --lib --locked -E '<filter>'`.
- Validator and config tests are in-crate unit tests and need no Docker. Do not run the Docker suites for this plan; `--all-targets` clippy compiles them.
- Chart scripts run under `/bin/bash` (3.2) on this Mac: `/bin/bash charts/paigasus/tests/<script>.sh --set ingress.host=console.example.test`. No `mapfile`, no `declare -A`, no here-string, no pipe for a render (renders go to `$TMP` files).
- The chart golden files (`charts/paigasus/tests/golden/*.yaml`) must stay byte-identical. Never run `render.sh --update`.
- Run the chart scripts with the worktree root as the current directory, and check `helm version --short` prints `v3.22.0+g144ca65`. Outside the repo the proto shim resolves helm 4.3.0 (no `.prototools`), and `render.sh` then reports one extra blank line per manifest against every golden file (MEASURED during planning). That red is the helm version, not the chart.
- Do not edit `charts/paigasus/templates/_helpers.tpl` (its whole-file copies live in `ci/helm-render/fixtures/`).
- Do not touch `ts/`, `py/`, `contracts/` or any lockfile.
- Commits: conventional commits with an allowed scope (`ts/packages/commitlint-config/index.cjs:42`). Rust tasks use `feat(rs): …`. Chart tasks use `feat(repo): …` (as SMA-691, SMA-694, SMA-695, SMA-697 did). The runbook task uses `docs(repo): …`. Header at most 100 characters; body lines at most 100 characters.
- No body line may start with a `word: value` shape or hold `#NNN`: commitlint then reads it as a footer and fails `footer-leading-blank`.
- End every commit message with a blank line and then `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Stage exact paths only (`git add <path> …`). Never `git add -A`, never `--no-verify`, never `git commit --amend`, never `git reset`, never `git stash`, never `git checkout -- <file>`.
- If `git commit` fails with `failed to fill whole buffer`, 1Password is locked: stop and ask the user to unlock it.
- If the `commit-msg` hook fails with `commitlint not found`, run `proto install` and then `pnpm -C ts install` (this changes no tracked file), and commit again.
- Text in docs, comments and messages uses ASD-STE100 Simplified Technical English: short sentences, active voice, no idiom.

## Review Focus

Five input classes that the spec implies, that no spec test names, and that would most likely bite
a user. Each line has its pinning test in the owning task.

1. **`--set oidc.idTokenMarkerClaims={}` as a rollback.** Helm 3.22.0 makes this `[""]` (MEASURED
   during planning). Expected: the render fails with `oidc.idTokenMarkerClaims[0] is empty`; the
   rollback forms are `=null` and `[]` in a values file. Pinned by `refusals.sh` row
   `markers set to {}` and `env.sh` rows `M2` and `M4` (Task 3), and by the runbook text (Task 4).
2. **A name with figment's separators.** The chart lets `[ ] { } , = #` through inside a quoted
   name. Expected: figment reads them as part of the string, and IAM boots. Pinned by the third
   case of `issuers_env_in_the_chart_form_parses_id_token_marker_claims` (Task 1).
3. **The defect order on the strict path.** `jsonwebtoken::decode` (11.1.0,
   `src/decoding.rs:287-288`) deserializes the caller's type BEFORE it validates `exp` and `aud`.
   Expected: a wrong-shaped claim stays `Malformed` ahead of `Expired` and `AudienceMismatch`, the
   same as on the plain path, and `aud: null` stays `AudienceMismatch`. Pinned by
   `strict_path_keeps_the_defect_order` (Task 2).
4. **The Zitadel recipe on the wrong IdP.** A Keycloak access token carries `azp`. Expected: IAM
   refuses every such token as `NotAnAccessToken`, and the log names `claim azp` and no claim
   value. Pinned by `wrong_recipe_fails_closed_and_names_the_claim` (Task 2).
5. **A misspelled key in a raw `iam.toml` or env value** (`id_token_marker_claim`). Expected: the
   value parses to an empty list with no error, IAM boots, and no boot line shows. Pinned by
   `a_misspelled_marker_key_is_ignored` (Task 1) and the second half of
   `boot_line_names_the_issuer_and_the_marker_claims` (Task 2).

## Facts confirmed in the code (planning, 2026-10-02)

- `rs/Cargo.lock`: `jsonwebtoken` 11.1.0. `decode<T: DeserializeOwned>` (`src/decoding.rs:270`)
  needs no `Clone`. A serde error in `T` becomes `ErrorKind::Json`, and `map_jwt_error`
  (`validator.rs:229-239`) maps it to `Malformed` through the `_` arm.
- A derived `Deserialize` refuses a repeated KNOWN field and ignores a repeated UNKNOWN member.
  So today `{"at_hash":"x","at_hash":null}` passes, and a repeated `cnf` is `Malformed`. The
  `StrictPayload` below was run in a scratch copy of the crate: all new and old validator and
  config tests pass, `cargo fmt --check` and `cargo clippy --all-targets -D warnings` are clean,
  and three mutations (step 6b removed, the duplicate check removed, `null` counted as a marker)
  each red the expected tests.
- `IamConfig::validate` runs before `paigasus_logging::init` (`src/main.rs:63-64`). So the boot
  line lives in `OidcAuthenticator::new`, which `AppState::new` calls once
  (`src/adapters/http/mod.rs:764`, `:780`).
- `IssuerConfig { … }` literals: `src/adapters/oidc/validator.rs:506`,
  `tests/support/mod.rs:483`, `tests/keycloak_e2e.rs:305`, `tests/authn_private_ca.rs:35`.
  `src/config.rs` has none.
- figment 0.10.19 reads a quoted string up to the next unescaped `"`
  (`src/value/parse.rs:28-41`), so `[ ] { } , = #` inside quotes are part of the name. If the
  whole value does not parse, figment keeps it as one raw string and the extract fails.
- Go `%q` writes a printable ASCII character as itself, and `\x01`, `\t`, `\x7f` as escapes; it
  keeps a printable non-ASCII rune (`"azpé"`). The chart regex `^[!#-\[\]-~]+$` admits exactly
  printable ASCII except space, `"` and `\` (MEASURED with helm 3.22.0).
- With the draft chart change, `render.sh`, `refusals.sh` and `env.sh` passed under `/bin/bash`
  3.2 on a scratch copy of the chart, and the goldens stayed byte-identical.

---

### Task 1: The `id_token_marker_claims` config field and its boot rules

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/config.rs:175-185` (field and constant), `:1050-1053` (validate loop), `:1503-1505` (new tests before `bootstrap_admins_env_in_the_chart_form_parses`)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs:505-511` (test helper literal)
- Modify: `rs/crates/services/paigasus-iam/tests/support/mod.rs:483-487`
- Modify: `rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs:305-312`
- Modify: `rs/crates/services/paigasus-iam/tests/authn_private_ca.rs:35-39`

**Interfaces:**
- Consumes: nothing new.
- Produces: `pub struct IssuerConfig { …, pub id_token_marker_claims: Vec<String> }` with `#[serde(default)]`; `const RESERVED_MARKER_CLAIMS: [&str; 4]` (private to `config.rs`); new `IamConfig::validate` errors that start with `authn.issuers[<issuer>].id_token_marker_claims`.

- [ ] **Step 1: Write the failing config tests**

In `rs/crates/services/paigasus-iam/src/config.rs` (`mod tests`), insert four tests directly
above `bootstrap_admins_env_in_the_chart_form_parses`. The `new_string` repeats that test's
`#[test]` line and signature at its end, so the existing test stays unchanged.

Use the Edit tool with this `old_string`:

```rust
    #[test]
    fn bootstrap_admins_env_in_the_chart_form_parses() {
```

and this `new_string`:

```rust
    #[test]
    fn issuers_env_in_the_chart_form_parses_id_token_marker_claims() {
        // SMA-703 T17: the exact string that charts/paigasus renders into IAM_AUTHN__ISSUERS with
        // oidc.idTokenMarkerClaims set (env.sh row M3), the same string without the key (row M1),
        // and Review Focus 2: the chart allows figment's separators `[ ] { } , =` and `#` inside a
        // quoted name, so figment must read them as part of the string.
        let cases: [(&str, Vec<&str>); 3] = [
            (
                r#"[{issuer="https://idp.example.test/realms/paigasus",audiences=["393381921683406851"],id_token_marker_claims=["at_hash","azp"]}]"#,
                vec!["at_hash", "azp"],
            ),
            (r#"[{issuer="https://idp.example.test/realms/paigasus",audiences=["393381921683406851"]}]"#, vec![]),
            (
                r#"[{issuer="https://idp.example.test/realms/paigasus",audiences=["paigasus-console"],id_token_marker_claims=["a[0]","b]x","{c},d=e#"]}]"#,
                vec!["a[0]", "b]x", "{c},d=e#"],
            ),
        ];
        for (issuers, want) in cases {
            figment::Jail::expect_with(|jail| {
                jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
                jail.set_env("IAM_API_KEYS__PEPPER", valid_pepper_b64());
                jail.set_env("IAM_AUTHN__ISSUERS", issuers);
                let cfg: IamConfig = IamConfig::figment().extract()?;
                assert_eq!(cfg.authn.issuers.len(), 1);
                assert_eq!(cfg.authn.issuers[0].id_token_marker_claims, want, "{issuers}");
                assert!(cfg.validate().is_ok(), "the chart's issuer string must pass validation: {issuers}");
                Ok(())
            });
        }
    }

    #[test]
    fn a_misspelled_marker_key_is_ignored() {
        // SMA-703 D2 / Review Focus 5: IssuerConfig ignores an unknown key, so a typo gives an
        // empty list and no error. The boot line in validator.rs is then absent; that is the
        // only sign. This test pins the fact the runbook relies on.
        figment::Jail::expect_with(|jail| {
            jail.set_env("IAM_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.set_env("IAM_API_KEYS__PEPPER", valid_pepper_b64());
            jail.set_env(
                "IAM_AUTHN__ISSUERS",
                r#"[{issuer="https://idp.example.test/realms/paigasus",audiences=["paigasus-console"],id_token_marker_claim=["at_hash"]}]"#,
            );
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.authn.issuers[0].id_token_marker_claims.is_empty());
            assert!(cfg.validate().is_ok());
            Ok(())
        });
    }

    #[test]
    fn validate_refuses_bad_id_token_marker_claims() {
        // SMA-703 T16: each D2 rule fails validate, and the message names the issuer.
        let cases: [(&[&str], &str); 9] = [
            (&[""], "contains an empty name"),
            (&[" at_hash"], "leading or trailing whitespace"),
            (&["azp\n"], "leading or trailing whitespace"),
            (&[" "], "leading or trailing whitespace"),
            (&["iss"], "must not contain \"iss\""),
            (&["sub"], "must not contain \"sub\""),
            (&["aud"], "must not contain \"aud\""),
            (&["exp"], "must not contain \"exp\""),
            (&["at_hash", "azp", "at_hash"], "contains the name \"at_hash\" twice"),
        ];
        for (names, want) in cases {
            let mut cfg = load_minimal_config();
            cfg.authn.issuers[0].id_token_marker_claims = names.iter().map(|name| (*name).to_string()).collect();
            let err = cfg.validate().expect_err("a bad marker list must fail validation");
            assert!(
                err.contains("authn.issuers[https://idp.example.com/realms/acme].id_token_marker_claims"),
                "{names:?}: the message names the issuer and the key: {err}"
            );
            assert!(err.contains(want), "{names:?}: want {want:?} in {err}");
        }
    }

    #[test]
    fn validate_accepts_marker_claims_and_compares_names_exactly() {
        // SMA-703 D2: the Zitadel recipe passes, and names differing only in case are two names.
        for names in [vec!["at_hash", "azp"], vec!["at_hash", "AT_HASH"], vec!["ISS"], vec![]] {
            let mut cfg = load_minimal_config();
            cfg.authn.issuers[0].id_token_marker_claims = names.iter().map(|name| (*name).to_string()).collect();
            assert!(cfg.validate().is_ok(), "{names:?} must pass validation: {:?}", cfg.validate());
        }
    }

    #[test]
    fn bootstrap_admins_env_in_the_chart_form_parses() {
```

- [ ] **Step 2: Run the tests and see them fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-703-zitadel/rs
cargo nextest run -p paigasus-iam --lib --locked -E 'test(/marker/)'
```

Expected: the build fails with `error[E0609]: no field `id_token_marker_claims` on type `IssuerConfig``
(and `E0609`/`E0560` at each use). No test runs.

- [ ] **Step 3: Add the field and the constant**

In `rs/crates/services/paigasus-iam/src/config.rs`, Edit `old_string`:

```rust
    #[serde(default = "default_jit_provisioning")]
    pub jit_provisioning: bool,
}

fn default_jit_provisioning() -> bool {
    true
}
```

`new_string`:

```rust
    #[serde(default = "default_jit_provisioning")]
    pub jit_provisioning: bool,
    /// Claim names that mark a verified token as NOT an access token for this issuer (SMA-703).
    /// Empty by default. A token that carries one of them, with any value except JSON `null`, is
    /// refused as `NotAnAccessToken`. For Zitadel, use `["at_hash", "azp"]` (spec § 3, F6).
    #[serde(default)]
    pub id_token_marker_claims: Vec<String>,
}

fn default_jit_provisioning() -> bool {
    true
}

/// Claim names that every token IAM accepts carries (SMA-703 D2). A marker list that names one
/// would make IAM refuse every token of that issuer, so `IamConfig::validate` refuses it.
const RESERVED_MARKER_CLAIMS: [&str; 4] = ["iss", "sub", "aud", "exp"];
```

- [ ] **Step 4: Add the boot rules to `validate`**

In the same file, Edit `old_string`:

```rust
            if let Err(e) = Issuer::parse(&issuer_cfg.issuer) {
                return Err(format!("authn.issuers[{trimmed}] is not a valid issuer: {e}"));
            }
        }
```

`new_string`:

```rust
            if let Err(e) = Issuer::parse(&issuer_cfg.issuer) {
                return Err(format!("authn.issuers[{trimmed}] is not a valid issuer: {e}"));
            }
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

Then add one sentence to the end of the `validate` doc comment. Edit `old_string`:

```rust
    /// key. Also (SMA-558): `authn.accept_invalid_tls` and `authn.extra_ca_bundle_path` are
    /// mutually exclusive, and the latter is non-empty when present.
```

`new_string`:

```rust
    /// key. Also (SMA-558): `authn.accept_invalid_tls` and `authn.extra_ca_bundle_path` are
    /// mutually exclusive, and the latter is non-empty when present. Also (SMA-703 D2): each
    /// `id_token_marker_claims` name is not empty, has no leading or trailing whitespace, is not
    /// `iss`/`sub`/`aud`/`exp`, and occurs once in its list.
```

- [ ] **Step 5: Update the four `IssuerConfig` literals**

`rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs` (test helper `issuer_config`), Edit `old_string`:

```rust
            audiences: audiences.iter().map(|a| (*a).to_string()).collect(),
            jit_provisioning: true,
        }
    }
```

`new_string`:

```rust
            audiences: audiences.iter().map(|a| (*a).to_string()).collect(),
            jit_provisioning: true,
            id_token_marker_claims: Vec::new(),
        }
    }
```

`rs/crates/services/paigasus-iam/tests/support/mod.rs`, Edit `old_string`:

```rust
                    jit_provisioning: *jit_provisioning,
```

`new_string`:

```rust
                    jit_provisioning: *jit_provisioning,
                    id_token_marker_claims: Vec::new(),
```

`rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs`, Edit `old_string`:

```rust
                audiences: vec!["paigasus".to_string(), "paigasus-cli".to_string()],
                jit_provisioning: true,
```

`new_string`:

```rust
                audiences: vec!["paigasus".to_string(), "paigasus-cli".to_string()],
                jit_provisioning: true,
                id_token_marker_claims: Vec::new(),
```

`rs/crates/services/paigasus-iam/tests/authn_private_ca.rs`, Edit `old_string`:

```rust
            jit_provisioning: true,
        }],
```

`new_string`:

```rust
            jit_provisioning: true,
            id_token_marker_claims: Vec::new(),
        }],
```

- [ ] **Step 6: Run the tests and see them pass**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-703-zitadel/rs
cargo nextest run -p paigasus-iam --lib --locked -E 'test(/marker|config::tests::issuers_env|config::tests::authn_defaults/)'
```

Expected: PASS for `issuers_env_in_the_chart_form_parses_id_token_marker_claims`,
`a_misspelled_marker_key_is_ignored`, `validate_refuses_bad_id_token_marker_claims`,
`validate_accepts_marker_claims_and_compares_names_exactly`,
`issuers_env_in_the_chart_form_parses_a_uri_and_a_digit_audience` and
`authn_defaults_land_with_a_minimal_issuer`.

- [ ] **Step 7: Run the whole lib suite, fmt and clippy**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-703-zitadel/rs
cargo nextest run -p paigasus-iam --lib --locked
cargo fmt --check -p paigasus-iam
cargo clippy -p paigasus-iam --all-targets --locked -- -D warnings
```

Expected: all lib tests PASS; `cargo fmt --check` prints nothing; clippy exits 0 (it compiles the
three integration-test files with the new literals).

- [ ] **Step 8: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-703-zitadel
git add rs/crates/services/paigasus-iam/src/config.rs \
  rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs \
  rs/crates/services/paigasus-iam/tests/support/mod.rs \
  rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs \
  rs/crates/services/paigasus-iam/tests/authn_private_ca.rs
git commit -m "feat(rs): add per-issuer id_token_marker_claims to the IAM config (SMA-703)" \
  -m "IssuerConfig gets an optional list of claim names, empty by default. IamConfig::validate
refuses an empty name, a padded name, iss, sub, aud or exp, and a repeated name, and the
message names the issuer. The figment tests parse the exact form that the chart renders." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git log --oneline -1
```

---

### Task 2: The validator refuses a token by a configured claim

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs:3-13` (module doc), `:56-73` (`RefusalDetail`, `ConfiguredIssuer`), `:89-107` (`new`), `:138-146` (`log_refusal`), `:223` (after `WireClaims`: `StrictPayload`, `configured_marker`), `:322-359` (`authenticate` steps 5-7), `:1105-1106` (new tests at the end of `mod tests`)

**Interfaces:**
- Consumes: `IssuerConfig::id_token_marker_claims: Vec<String>` (Task 1); existing test helpers `es256_keypair`, `sign`, `make_authenticator`, `issuer_config`, `StubFetcher`, `ISSUER`, `capture_logs`.
- Produces (all private to `validator.rs`): `RefusalDetail::Claim(&'a str)`; `ConfiguredIssuer::id_token_marker_claims: Vec<String>`; `struct StrictPayload { members: serde_json::Map<String, serde_json::Value>, claims: WireClaims }` with `impl<'de> Deserialize<'de>`; `fn configured_marker<'a>(members: &serde_json::Map<String, serde_json::Value>, names: &'a [String]) -> Option<&'a str>`; the boot log message `IAM refuses a verified token of this issuer that carries one of the configured ID-token marker claims`; the refusal marker text `claim <name>`.

- [ ] **Step 1: Write the failing validator tests**

In `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs`, Edit `old_string` (the end of
the file):

```rust
        let text = logs.text();
        assert_eq!(text.lines().filter(|line| line.contains(BINDING_REFUSAL)).count(), 1, "binding lines:\n{text}");
    }
}
```

`new_string`:

```rust
        let text = logs.text();
        assert_eq!(text.lines().filter(|line| line.contains(BINDING_REFUSAL)).count(), 1, "binding lines:\n{text}");
    }

    // ---- SMA-703: configured ID-token marker claims ----------------------------------------
    //
    // The Zitadel fixtures copy the claim NAMES and value shapes of the measured tokens
    // (docs/superpowers/specs/2026-10-02-sma-703-zitadel-measurements.md, M1, M4a, M5b). The
    // times are relative to `Utc::now()`, because the measured `exp` values end on 2026-10-03.
    // `iss` is the test issuer. `aud` stays an array, so `WireAudience::Multiple` runs.

    /// The runbook recipe for Zitadel (spec D6).
    const ZITADEL_MARKERS: [&str; 2] = ["at_hash", "azp"];
    /// Measured ids: project P, the extra `aud` id (INFERRED: the app id of A), the client id of
    /// web app A, the human subject, the machine client id and the machine subject.
    const ZITADEL_PROJECT_ID: &str = "393381921683406851";
    const ZITADEL_APP_ID: &str = "393381921700315139";
    const ZITADEL_CLIENT_ID: &str = "393381921750515715";
    const ZITADEL_HUMAN_SUB: &str = "393381921784070147";
    const ZITADEL_MACHINE_CLIENT_ID: &str = "sma703-svc";
    const ZITADEL_MACHINE_SUB: &str = "393381990419660803";
    /// A second configured issuer, for the per-issuer test (T14).
    const SECOND_ISSUER: &str = "https://idp2.example.com";
    /// The SMA-686 refusal message, shared by the configured-claim refusal (spec D4).
    const NOT_ACCESS_TOKEN_REFUSAL: &str = "a verified marker shows it is not an access token";
    /// The SMA-703 boot line (spec D2).
    const MARKER_BOOT_LINE: &str = "carries one of the configured ID-token marker claims";

    /// The human-flow access token of M1 (and of the M4a refresh, with its own `jti`).
    fn zitadel_human_access_token(jti: &str) -> serde_json::Value {
        let now = Utc::now().timestamp();
        serde_json::json!({
            "iss": ISSUER,
            "sub": ZITADEL_HUMAN_SUB,
            "aud": [ZITADEL_APP_ID, ZITADEL_CLIENT_ID, ZITADEL_PROJECT_ID],
            "exp": now + 3600,
            "iat": now,
            "nbf": now,
            "client_id": ZITADEL_CLIENT_ID,
            "jti": jti,
        })
    }

    /// The human-flow ID token of M1 (and of the M4a refresh, with its own `at_hash`).
    fn zitadel_human_id_token(at_hash: &str) -> serde_json::Value {
        let now = Utc::now().timestamp();
        serde_json::json!({
            "iss": ISSUER,
            "sub": ZITADEL_HUMAN_SUB,
            "aud": [ZITADEL_APP_ID, ZITADEL_CLIENT_ID, ZITADEL_PROJECT_ID],
            "exp": now + 3600,
            "iat": now,
            "auth_time": now - 4,
            "nonce": "58e866bad23abebb",
            "amr": ["pwd"],
            "azp": ZITADEL_CLIENT_ID,
            "client_id": ZITADEL_CLIENT_ID,
            "at_hash": at_hash,
            "sid": "V1_393381929921019907",
        })
    }

    fn zitadel_m1_access_token() -> serde_json::Value {
        zitadel_human_access_token("V2_393381935289729027-at_393381935289794563")
    }

    fn zitadel_m1_id_token() -> serde_json::Value {
        zitadel_human_id_token("FFPzlMOE6pZPHZWKKJOObg")
    }

    fn zitadel_m4_access_token() -> serde_json::Value {
        zitadel_human_access_token("V2_393381935289729027-at_393381935306571779")
    }

    fn zitadel_m4_id_token() -> serde_json::Value {
        zitadel_human_id_token("9yO4kdn1jViUdIJ1kQrkjw")
    }

    /// The machine (client-credentials) access token of M5b: scope `openid` plus the P aud scope.
    fn zitadel_m5b_access_token() -> serde_json::Value {
        let now = Utc::now().timestamp();
        serde_json::json!({
            "iss": ISSUER,
            "sub": ZITADEL_MACHINE_SUB,
            "aud": [ZITADEL_PROJECT_ID],
            "exp": now + 3600,
            "iat": now,
            "nbf": now,
            "client_id": ZITADEL_MACHINE_CLIENT_ID,
            "jti": "V2_393381990436569091-at_393381990436634627",
        })
    }

    /// The machine ID token of M5b. No `nonce` and no `sid`: the client sent no nonce (F7).
    fn zitadel_m5b_id_token() -> serde_json::Value {
        let now = Utc::now().timestamp();
        serde_json::json!({
            "iss": ISSUER,
            "sub": ZITADEL_MACHINE_SUB,
            "aud": [ZITADEL_PROJECT_ID, ZITADEL_MACHINE_CLIENT_ID],
            "exp": now + 3600,
            "iat": now,
            "auth_time": now,
            "amr": ["pwd"],
            "azp": ZITADEL_MACHINE_CLIENT_ID,
            "client_id": ZITADEL_MACHINE_CLIENT_ID,
            "at_hash": "kYHj1WCq97WeD6uAjPviug",
        })
    }

    /// `base` with the members of `extra` added or replaced.
    fn merged(mut base: serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
        let extra = extra.as_object().expect("extra claims are a JSON object").clone();
        base.as_object_mut().expect("claims are a JSON object").extend(extra);
        base
    }

    /// `base` without the member `name`.
    fn without(mut base: serde_json::Value, name: &str) -> serde_json::Value {
        base.as_object_mut().expect("claims are a JSON object").remove(name);
        base
    }

    fn issuer_with_markers(issuer: &str, audiences: &[&str], markers: &[&str]) -> IssuerConfig {
        IssuerConfig {
            id_token_marker_claims: markers.iter().map(|marker| (*marker).to_string()).collect(),
            ..issuer_config(issuer, audiences)
        }
    }

    /// Signs `claims` with a fresh key and authenticates it against `ISSUER`, configured with the
    /// audience `ZITADEL_PROJECT_ID` (runbook option 1) and the marker claims `markers`.
    async fn authenticate_zitadel(markers: &[&str], claims: &serde_json::Value) -> Result<ValidatedClaims, AuthnError> {
        let (encoding_key, jwk, kid) = es256_keypair();
        let token = sign(&encoding_key, Some(&kid), claims);
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_with_markers(ISSUER, &[ZITADEL_PROJECT_ID], markers)], 60, 16_384);
        authenticator.authenticate(&token).await
    }

    fn assert_not_an_access_token(result: Result<ValidatedClaims, AuthnError>, name: &str) {
        match result {
            Err(AuthnError::InvalidToken(TokenDefect::NotAnAccessToken)) => {}
            other => panic!("{name}: must be refused as NotAnAccessToken, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn refuses_zitadel_human_id_token() {
        // Spec § 5 T1.
        assert_not_an_access_token(authenticate_zitadel(&ZITADEL_MARKERS, &zitadel_m1_id_token()).await, "M1 ID token");
    }

    #[tokio::test]
    async fn refuses_zitadel_machine_id_token() {
        // Spec § 5 T2. The machine ID token has no nonce, so only the configured names refuse it.
        let claims = zitadel_m5b_id_token();
        assert!(claims.get("nonce").is_none(), "the M5b fixture has no nonce");
        assert_not_an_access_token(authenticate_zitadel(&ZITADEL_MARKERS, &claims).await, "M5b ID token");
    }

    #[tokio::test]
    async fn accepts_zitadel_access_tokens() {
        // Spec § 5 T3.
        let human = authenticate_zitadel(&ZITADEL_MARKERS, &zitadel_m1_access_token()).await.expect("the M1 access token must be accepted");
        assert_eq!(human.subject, ZITADEL_HUMAN_SUB);
        assert_eq!(human.audiences, vec![ZITADEL_APP_ID.to_string(), ZITADEL_CLIENT_ID.to_string(), ZITADEL_PROJECT_ID.to_string()]);
        let machine = authenticate_zitadel(&ZITADEL_MARKERS, &zitadel_m5b_access_token())
            .await
            .expect("the M5b access token must be accepted");
        assert_eq!(machine.subject, ZITADEL_MACHINE_SUB);
        assert_eq!(machine.audiences, vec![ZITADEL_PROJECT_ID.to_string()]);
    }

    #[tokio::test]
    async fn refresh_grant_tokens_keep_their_kind() {
        // Spec § 5 T4: the M4a refresh returns a new ID token and a new access token.
        assert_not_an_access_token(authenticate_zitadel(&ZITADEL_MARKERS, &zitadel_m4_id_token()).await, "M4 ID token");
        authenticate_zitadel(&ZITADEL_MARKERS, &zitadel_m4_access_token()).await.expect("the M4 access token must be accepted");
    }

    #[tokio::test]
    async fn empty_marker_list_keeps_the_sma_686_behaviour() {
        // Spec § 5 T5 and T6 (G2): with no configured names, a Zitadel ID token is accepted (the
        // open state that the setting closes), and so is the Dex shape.
        authenticate_zitadel(&[], &zitadel_m1_id_token())
            .await
            .expect("with an empty list the M1 ID token is accepted (open state)");
        let dex = merged(zitadel_m1_access_token(), serde_json::json!({ "at_hash": "x", "c_hash": "y", "nonce": "abc123" }));
        authenticate_zitadel(&[], &dex).await.expect("with an empty list the Dex shape is accepted");
    }

    #[tokio::test]
    async fn null_value_is_not_a_marker_and_any_other_value_is() {
        // Spec § 5 T7: the SMA-690 `cnf` rule.
        let null = merged(zitadel_m1_access_token(), serde_json::json!({ "at_hash": null }));
        authenticate_zitadel(&["at_hash"], &null).await.expect("at_hash: null is not a marker");
        for value in [serde_json::json!(""), serde_json::json!(0), serde_json::json!({}), serde_json::json!([]), serde_json::json!(false)] {
            let claims = merged(zitadel_m1_access_token(), serde_json::json!({ "at_hash": value.clone() }));
            assert_not_an_access_token(authenticate_zitadel(&["at_hash"], &claims).await, &format!("at_hash: {value}"));
        }
    }

    #[tokio::test]
    async fn only_configured_names_count() {
        // Spec § 5 T8: `azp` is present, but only `at_hash` is configured.
        let claims = without(zitadel_m1_id_token(), "at_hash");
        assert!(claims.get("azp").is_some(), "the fixture keeps azp");
        authenticate_zitadel(&["at_hash"], &claims).await.expect("an unconfigured name is not a marker");
    }

    #[tokio::test]
    async fn signature_and_claims_defects_come_first() {
        // Spec § 5 T9 (D3): `decode` validates before the marker check.
        let expired = merged(zitadel_m1_id_token(), serde_json::json!({ "exp": Utc::now().timestamp() - 120 }));
        let err = authenticate_zitadel(&ZITADEL_MARKERS, &expired).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::Expired)), "got {err:?}");
        let wrong_aud = merged(zitadel_m1_id_token(), serde_json::json!({ "aud": ["other-project"] }));
        let err = authenticate_zitadel(&ZITADEL_MARKERS, &wrong_aud).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::AudienceMismatch)), "got {err:?}");
    }

    #[tokio::test]
    async fn keycloak_typ_marker_runs_before_the_configured_claims() {
        // Spec § 5 T10: the SMA-686 marker wins, and the log names `ID`, not `claim at_hash`.
        let (logs, _guard) = capture_logs();
        let claims = merged(zitadel_m1_access_token(), serde_json::json!({ "typ": "ID", "at_hash": "x" }));
        assert_not_an_access_token(authenticate_zitadel(&ZITADEL_MARKERS, &claims).await, "typ ID with at_hash");
        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|line| line.contains(NOT_ACCESS_TOKEN_REFUSAL)).collect();
        assert_eq!(lines.len(), 1, "exactly one refusal line expected, got:\n{text}");
        assert!(lines[0].contains("\"ID\"") || lines[0].contains("=ID"), "the marker is ID: {}", lines[0]);
        assert!(!text.contains("claim at_hash"), "the configured claim must not be the logged marker:\n{text}");
    }

    #[tokio::test]
    async fn configured_claim_refusal_logs_issuer_and_claim_name_only() {
        // Spec § 5 T11 (D4): issuer and `claim at_hash`; no claim value, subject or email; one
        // line for three refusals (the SMA-686 D14 rate limit).
        let (logs, _guard) = capture_logs();
        let (encoding_key, jwk, kid) = es256_keypair();
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_with_markers(ISSUER, &[ZITADEL_PROJECT_ID], &ZITADEL_MARKERS)], 60, 16_384);
        let claims = merged(zitadel_m1_id_token(), serde_json::json!({ "email": "alice@example.com" }));
        for _ in 0..3 {
            let token = sign(&encoding_key, Some(&kid), &claims);
            assert_not_an_access_token(authenticator.authenticate(&token).await, "M1 ID token");
        }
        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|line| line.contains(NOT_ACCESS_TOKEN_REFUSAL)).collect();
        assert_eq!(lines.len(), 1, "exactly one refusal line expected, got:\n{text}");
        let line = lines[0];
        assert!(line.contains("INFO"), "the refusal logs at info: {line}");
        assert!(line.contains(ISSUER), "the refusal names the issuer: {line}");
        assert!(line.contains("claim at_hash"), "the refusal names the first configured claim: {line}");
        for secret in ["FFPzlMOE6pZPHZWKKJOObg", ZITADEL_HUMAN_SUB, ZITADEL_CLIENT_ID, "58e866bad23abebb", "alice@example.com"] {
            assert!(!text.contains(secret), "the log must not contain {secret:?}:\n{text}");
        }
    }

    #[tokio::test]
    async fn wrong_recipe_fails_closed_and_names_the_claim() {
        // Review Focus 4 (spec § 6): the Zitadel recipe on an IdP whose access token carries `azp`
        // (a Keycloak access token: `typ: Bearer`, `azp`, no `at_hash`) refuses that access token.
        // The log names `claim azp`, so the operator sees which name is wrong.
        let (logs, _guard) = capture_logs();
        let keycloak_access = merged(zitadel_m1_access_token(), serde_json::json!({ "typ": "Bearer", "azp": "paigasus-console" }));
        assert_not_an_access_token(authenticate_zitadel(&ZITADEL_MARKERS, &keycloak_access).await, "Keycloak access token");
        let text = logs.text();
        assert!(text.contains("claim azp"), "the log names the matched claim:\n{text}");
        assert!(!text.contains("paigasus-console"), "the log must not contain the azp value:\n{text}");
    }

    /// Signs a raw payload JSON string by hand, so a test can repeat a member name (neither
    /// `json!` nor `jsonwebtoken::encode` can emit a repeated key).
    fn sign_raw_payload(encoding_key: &EncodingKey, kid: &str, payload_json: &str) -> String {
        let header_json = format!(r#"{{"alg":"ES256","typ":"JWT","kid":"{kid}"}}"#);
        let header_b64 = URL_SAFE_NO_PAD.encode(header_json.as_bytes());
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
        let message = format!("{header_b64}.{payload_b64}");
        let signature = jsonwebtoken::crypto::sign(message.as_bytes(), encoding_key, Algorithm::ES256).expect("signing a test token");
        format!("{message}.{signature}")
    }

    /// Authenticates the raw `payload` JSON string against `ISSUER`, configured with the audience
    /// `aud` and the marker claims `markers`.
    async fn authenticate_raw_payload(markers: &[&str], payload: &str) -> Result<ValidatedClaims, AuthnError> {
        let (encoding_key, jwk, kid) = es256_keypair();
        let token = sign_raw_payload(&encoding_key, &kid, payload);
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_with_markers(ISSUER, &["aud"], markers)], 60, 16_384);
        authenticator.authenticate(&token).await
    }

    /// Authenticates a payload of the usual test fields (`ISSUER`, aud `aud`, one hour) followed
    /// by `members` verbatim, against `ISSUER` configured with `markers`.
    async fn authenticate_raw(markers: &[&str], members: &str) -> Result<ValidatedClaims, AuthnError> {
        let exp = Utc::now().timestamp() + 3600;
        let payload = format!(r#"{{"iss":"{ISSUER}","sub":"sub-1","aud":"aud","exp":{exp},"email":"alice@example.com",{members}}}"#);
        authenticate_raw_payload(markers, &payload).await
    }

    #[tokio::test]
    async fn duplicate_member_is_malformed_on_the_strict_path() {
        // Spec § 5 T12 (D3). A plain map would keep the last value, so `null` last would hide
        // the marker. Both orders are Malformed. A duplicate `cnf` is Malformed on both paths.
        for (name, markers, members) in [
            ("at_hash string then null", &ZITADEL_MARKERS[..], r#""at_hash":"x","at_hash":null"#),
            ("at_hash null then string", &ZITADEL_MARKERS[..], r#""at_hash":null,"at_hash":"x""#),
            ("cnf twice, list set", &ZITADEL_MARKERS[..], r#""cnf":{"jkt":"abc"},"cnf":null"#),
            ("cnf twice, list empty", &[][..], r#""cnf":{"jkt":"abc"},"cnf":null"#),
            ("sub twice, list set", &ZITADEL_MARKERS[..], r#""sub":"sub-2""#),
        ] {
            let err = authenticate_raw(markers, members).await.unwrap_err();
            assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::Malformed)), "{name}: got {err:?}");
        }
    }

    #[tokio::test]
    async fn duplicate_unknown_member_is_unchanged_on_the_plain_path() {
        // Spec § 7 / G2: with an empty list the SMA-686 decode runs unchanged, and that decode
        // ignores a repeated member it does not read. This pins that the new code did not move
        // an issuer without marker claims onto the strict path.
        authenticate_raw(&[], r#""at_hash":"x","at_hash":null"#).await.expect("the plain path is unchanged");
    }

    #[tokio::test]
    async fn strict_path_keeps_the_defect_order() {
        // Review Focus 3: `StrictPayload` reads `WireClaims` inside its own `Deserialize`, so a
        // wrong-shaped claim is Malformed BEFORE `jsonwebtoken` validates `exp` and `aud`, on both
        // paths. A parse after `decode` would turn `aud: 7` into AudienceMismatch (and log it),
        // and an expired token without `sub` into Expired.
        let future = Utc::now().timestamp() + 3600;
        let past = Utc::now().timestamp() - 120;
        let cases = [
            ("aud a number", format!(r#"{{"iss":"{ISSUER}","sub":"sub-1","aud":7,"exp":{future}}}"#), TokenDefect::Malformed),
            ("expired and no sub", format!(r#"{{"iss":"{ISSUER}","aud":"aud","exp":{past}}}"#), TokenDefect::Malformed),
            (
                "name a number",
                format!(r#"{{"iss":"{ISSUER}","sub":"sub-1","aud":"aud","exp":{future},"name":7}}"#),
                TokenDefect::Malformed,
            ),
            ("aud null", format!(r#"{{"iss":"{ISSUER}","sub":"sub-1","aud":null,"exp":{future}}}"#), TokenDefect::AudienceMismatch),
            ("expired", format!(r#"{{"iss":"{ISSUER}","sub":"sub-1","aud":"aud","exp":{past}}}"#), TokenDefect::Expired),
        ];
        for markers in [&ZITADEL_MARKERS[..], &[][..]] {
            for (name, payload, want) in &cases {
                let err = authenticate_raw_payload(markers, payload).await.unwrap_err();
                assert!(
                    matches!(&err, AuthnError::InvalidToken(defect) if defect == want),
                    "{name}, markers {markers:?}: want {want:?}, got {err:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn marker_names_are_case_sensitive() {
        // Spec § 5 T13 (D2).
        let claims = merged(zitadel_m1_access_token(), serde_json::json!({ "AT_HASH": "x" }));
        authenticate_zitadel(&["at_hash"], &claims).await.expect("AT_HASH is not at_hash");
    }

    #[tokio::test]
    async fn marker_list_is_per_issuer() {
        // Spec § 5 T14: one issuer with `["at_hash"]`, one with an empty list, one key for both.
        let (encoding_key, jwk, kid) = es256_keypair();
        let authenticator = make_authenticator(
            StubFetcher::new(jwk),
            vec![issuer_with_markers(ISSUER, &[ZITADEL_PROJECT_ID], &["at_hash"]), issuer_config(SECOND_ISSUER, &[ZITADEL_PROJECT_ID])],
            60,
            16_384,
        );
        let first = sign(&encoding_key, Some(&kid), &zitadel_m1_id_token());
        assert_not_an_access_token(authenticator.authenticate(&first).await, "ID token of the first issuer");
        let second = sign(&encoding_key, Some(&kid), &merged(zitadel_m1_id_token(), serde_json::json!({ "iss": SECOND_ISSUER })));
        let validated = authenticator.authenticate(&second).await.expect("the second issuer has no marker claims");
        assert_eq!(validated.issuer.as_str(), SECOND_ISSUER);
    }

    #[test]
    fn boot_line_names_the_issuer_and_the_marker_claims() {
        // Spec § 5 T15 (D2): one info line for an issuer with marker claims; none for an empty
        // list. `capture_logs` installs a thread-local subscriber; `new` is synchronous.
        let (logs, _guard) = capture_logs();
        let (_encoding_key, jwk, _kid) = es256_keypair();
        let _with = make_authenticator(
            StubFetcher::new(jwk.clone()),
            vec![issuer_with_markers(ISSUER, &["aud"], &ZITADEL_MARKERS), issuer_config(SECOND_ISSUER, &["aud"])],
            60,
            16_384,
        );
        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|line| line.contains(MARKER_BOOT_LINE)).collect();
        assert_eq!(lines.len(), 1, "exactly one boot line expected, got:\n{text}");
        let line = lines[0];
        assert!(line.contains("INFO"), "the boot line logs at info: {line}");
        assert!(line.contains(ISSUER), "the boot line names the issuer: {line}");
        assert!(line.contains("at_hash") && line.contains("azp"), "the boot line names the claims: {line}");
        assert!(!line.contains(SECOND_ISSUER), "the boot line names only the issuer with claims: {line}");

        let (logs, _guard) = capture_logs();
        let _without = make_authenticator(StubFetcher::new(jwk), vec![issuer_config(ISSUER, &["aud"])], 60, 16_384);
        assert!(!logs.text().contains(MARKER_BOOT_LINE), "no boot line for an empty list:\n{}", logs.text());
    }
}
```

- [ ] **Step 2: Run the tests and see the right ones fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-703-zitadel/rs
cargo nextest run -p paigasus-iam --lib --locked --no-fail-fast -E 'test(/validator::tests/)'
```

Expected: the build succeeds. These nine tests FAIL: `refuses_zitadel_human_id_token`,
`refuses_zitadel_machine_id_token`, `refresh_grant_tokens_keep_their_kind`,
`null_value_is_not_a_marker_and_any_other_value_is`,
`configured_claim_refusal_logs_issuer_and_claim_name_only`,
`wrong_recipe_fails_closed_and_names_the_claim`, `marker_list_is_per_issuer`,
`duplicate_member_is_malformed_on_the_strict_path` (the two `at_hash` cases authenticate) and
`boot_line_names_the_issuer_and_the_marker_claims` (0 boot lines). The other new tests already
pass, because they pin behaviour that must not change (G2, the defect order, case sensitivity,
the SMA-686 order). Every old validator test passes.

- [ ] **Step 3: Extend the module doc, `RefusalDetail` and `ConfiguredIssuer`**

Edit `old_string`:

```rust
//! back-channel logout marker (header `typ: logout+jwt`, the `events` member). The
//! sender-constraint check
```

`new_string`:

```rust
//! back-channel logout marker (header `typ: logout+jwt`, the `events` member). After those
//! markers, it refuses a token that carries a claim the operator named for the issuer in
//! `id_token_marker_claims` (SMA-703); for such an issuer the payload is also decoded as a map
//! that refuses a repeated top-level member (`StrictPayload`). The
//! sender-constraint check
```

Edit `old_string`:

```rust
    /// The static marker that shows a verified token is bound to a key (SMA-690 D8).
    Binding(&'static str),
}

/// One configured issuer, parsed once at construction — replacing the per-request
/// `Issuer::parse` the request path used to run after every issuer match.
/// `jit_provisioning` is deliberately absent: the validator never reads it.
struct ConfiguredIssuer {
    issuer: Issuer,
    audiences: Vec<String>,
}
```

`new_string`:

```rust
    /// The static marker that shows a verified token is bound to a key (SMA-690 D8).
    Binding(&'static str),
    /// The CONFIGURED claim name that shows a verified token is not an access token (SMA-703
    /// D4). Logged as the marker `claim <name>`.
    Claim(&'a str),
}

/// One configured issuer, parsed once at construction — replacing the per-request
/// `Issuer::parse` the request path used to run after every issuer match.
/// `jit_provisioning` is deliberately absent: the validator never reads it.
struct ConfiguredIssuer {
    issuer: Issuer,
    audiences: Vec<String>,
    /// SMA-703 D1: the configured claim names. Empty keeps the SMA-686 decode path unchanged.
    id_token_marker_claims: Vec<String>,
}
```

- [ ] **Step 4: Carry the list in `new`, and write the boot line**

Edit `old_string`:

```rust
    /// defect, mirroring the `redis_url` guard in `AppState::new`.
    pub fn new(issuers: Vec<IssuerConfig>, provider: JwksProvider<F, K, C>, leeway_secs: u64, max_token_bytes: usize) -> Result<Self, AuthnError> {
        let issuers = issuers
            .into_iter()
            .map(|cfg| {
                let issuer = Issuer::parse(&cfg.issuer).map_err(|e| AuthnError::Backend(e.to_string().into()))?;
                Ok(ConfiguredIssuer { issuer, audiences: cfg.audiences })
            })
```

`new_string`:

```rust
    /// defect, mirroring the `redis_url` guard in `AppState::new`.
    ///
    /// Writes one `info` line for each issuer with configured `id_token_marker_claims` (SMA-703
    /// D2). `AppState::new` calls this once at boot, after `paigasus_logging::init`;
    /// `IamConfig::validate` runs before the logger exists, so the line cannot live there.
    pub fn new(issuers: Vec<IssuerConfig>, provider: JwksProvider<F, K, C>, leeway_secs: u64, max_token_bytes: usize) -> Result<Self, AuthnError> {
        let issuers = issuers
            .into_iter()
            .map(|cfg| {
                let issuer = Issuer::parse(&cfg.issuer).map_err(|e| AuthnError::Backend(e.to_string().into()))?;
                if !cfg.id_token_marker_claims.is_empty() {
                    tracing::info!(
                        issuer = issuer.as_str(),
                        id_token_marker_claims = ?cfg.id_token_marker_claims,
                        "IAM refuses a verified token of this issuer that carries one of the configured ID-token marker claims"
                    );
                }
                Ok(ConfiguredIssuer {
                    issuer,
                    audiences: cfg.audiences,
                    id_token_marker_claims: cfg.id_token_marker_claims,
                })
            })
```

- [ ] **Step 5: Log the configured-claim refusal**

Edit `old_string`:

```rust
                    "refused a bearer token: it is bound to a key, and IAM cannot check the binding"
                );
            }
        }
    }
}
```

`new_string`:

```rust
                    "refused a bearer token: it is bound to a key, and IAM cannot check the binding"
                );
            }
            RefusalDetail::Claim(name) => {
                // The same message as `Marker`: an operator greps one text for SMA-686 and SMA-703.
                let marker = format!("claim {name}");
                tracing::info!(
                    issuer = issuer.as_str(),
                    marker = marker.as_str(),
                    suppressed,
                    "refused a bearer token: a verified marker shows it is not an access token"
                );
            }
        }
    }
}
```

- [ ] **Step 6: Add `StrictPayload` and `configured_marker`**

Edit `old_string`:

```rust
    cnf: Option<serde_json::Value>,
}

/// Maps a `jsonwebtoken` decode/validation failure to a `TokenDefect` (spec §4.1). Every
```

`new_string`:

```rust
    cnf: Option<serde_json::Value>,
}

/// The verified payload for an issuer with configured marker claims (SMA-703 D3): every top-level
/// member, plus the same `WireClaims` the plain path reads. A plain `serde_json::Map` keeps the
/// LAST value of a repeated key with no error, and a derived struct does not check a repeated
/// member it ignores, so `{"at_hash":"x","at_hash":null}` would pass both. This `Deserialize`
/// refuses a repeated top-level member name; `jsonwebtoken` reports that serde error as
/// `ErrorKind::Json`, and `map_jwt_error` maps it to `Malformed`.
///
/// `claims` is read INSIDE `deserialize`, not after `decode` returns. `jsonwebtoken::decode`
/// deserializes the caller's type before it validates `exp`/`aud`/`iss`, so a wrong-shaped claim
/// stays `Malformed` ahead of an expiry or audience defect, exactly as on the plain path.
struct StrictPayload {
    members: serde_json::Map<String, serde_json::Value>,
    claims: WireClaims,
}

impl<'de> Deserialize<'de> for StrictPayload {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueMembers;

        impl<'de> serde::de::Visitor<'de> for UniqueMembers {
            type Value = serde_json::Map<String, serde_json::Value>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON object with unique member names")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut members = serde_json::Map::new();
                while let Some(name) = access.next_key::<String>()? {
                    // The error text names no member: the payload is token material (SMA-686 D8).
                    if members.contains_key(&name) {
                        return Err(serde::de::Error::custom("a top-level member name occurs twice"));
                    }
                    let value = access.next_value::<serde_json::Value>()?;
                    members.insert(name, value);
                }
                Ok(members)
            }
        }

        let members = deserializer.deserialize_map(UniqueMembers)?;
        let claims = WireClaims::deserialize(serde_json::Value::Object(members.clone())).map_err(serde::de::Error::custom)?;
        Ok(StrictPayload { members, claims })
    }
}

/// The first configured claim name that the verified payload carries with a value other than
/// JSON `null`, or `None` (SMA-703 D3). Any other value is a marker: a string, a number, an
/// object, an array, an empty string (the SMA-690 rule for `cnf`). Names compare exactly, because
/// JSON member names are case-sensitive. The list order decides which name a log line shows.
fn configured_marker<'a>(members: &serde_json::Map<String, serde_json::Value>, names: &'a [String]) -> Option<&'a str> {
    names.iter().find(|name| members.get(name.as_str()).is_some_and(|value| !value.is_null())).map(String::as_str)
}

/// Maps a `jsonwebtoken` decode/validation failure to a `TokenDefect` (spec §4.1). Every
```

- [ ] **Step 7: Use the strict decode and the check in `authenticate`**

Edit `old_string`:

```rust
        let token_data = decode::<WireClaims>(token, &decoding_key, &validation).map_err(|err| {
            let err = map_jwt_error(err);
            if matches!(err, AuthnError::InvalidToken(TokenDefect::AudienceMismatch)) {
                self.log_refusal(&issuer, TokenDefect::AudienceMismatch, RefusalDetail::Accepted(&issuer_config.audiences));
            }
            err
        })?;

        // 6. Token-type check on the verified token (SMA-686): an ID token or a logout token.
        if let Some(marker) = non_access_token_marker(token_data.header.typ.as_deref(), &token_data.claims) {
            self.log_refusal(&issuer, TokenDefect::NotAnAccessToken, RefusalDetail::Marker(marker));
            return Err(invalid(TokenDefect::NotAnAccessToken));
        }

        // 7. Sender-constraint check on the verified token (SMA-690): IAM cannot check a binding.
        if let Some(marker) = sender_constraint_marker(&token_data.claims) {
            self.log_refusal(&issuer, TokenDefect::SenderConstrained, RefusalDetail::Binding(marker));
            return Err(invalid(TokenDefect::SenderConstrained));
        }

        let expires_at = i64::try_from(token_data.claims.exp)
            .ok()
            .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0))
            .ok_or_else(|| invalid(TokenDefect::Malformed))?;

        Ok(ValidatedClaims {
            issuer,
            subject: token_data.claims.sub,
            // A second guard: `validate` already refuses a missing `aud` (D13); if that ever
            // regresses, the token is still refused, as Malformed.
            audiences: token_data.claims.aud.map(WireAudience::into_vec).ok_or_else(|| invalid(TokenDefect::Malformed))?,
            expires_at,
            email: token_data.claims.email,
            name: token_data.claims.name,
            locale: token_data.claims.locale,
            zoneinfo: token_data.claims.zoneinfo,
        })
```

`new_string`:

```rust
        let on_decode_error = |err: jsonwebtoken::errors::Error| {
            let err = map_jwt_error(err);
            if matches!(err, AuthnError::InvalidToken(TokenDefect::AudienceMismatch)) {
                self.log_refusal(&issuer, TokenDefect::AudienceMismatch, RefusalDetail::Accepted(&issuer_config.audiences));
            }
            err
        };
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

        // 6. Token-type check on the verified token (SMA-686): an ID token or a logout token.
        if let Some(marker) = non_access_token_marker(verified_header.typ.as_deref(), &claims) {
            self.log_refusal(&issuer, TokenDefect::NotAnAccessToken, RefusalDetail::Marker(marker));
            return Err(invalid(TokenDefect::NotAnAccessToken));
        }
        // 6b. A configured marker claim (SMA-703 D3), after the SMA-686 markers.
        if let Some(name) = claim_marker {
            self.log_refusal(&issuer, TokenDefect::NotAnAccessToken, RefusalDetail::Claim(name));
            return Err(invalid(TokenDefect::NotAnAccessToken));
        }

        // 7. Sender-constraint check on the verified token (SMA-690): IAM cannot check a binding.
        if let Some(marker) = sender_constraint_marker(&claims) {
            self.log_refusal(&issuer, TokenDefect::SenderConstrained, RefusalDetail::Binding(marker));
            return Err(invalid(TokenDefect::SenderConstrained));
        }

        let expires_at = i64::try_from(claims.exp)
            .ok()
            .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0))
            .ok_or_else(|| invalid(TokenDefect::Malformed))?;

        Ok(ValidatedClaims {
            issuer,
            subject: claims.sub,
            // A second guard: `validate` already refuses a missing `aud` (D13); if that ever
            // regresses, the token is still refused, as Malformed.
            audiences: claims.aud.map(WireAudience::into_vec).ok_or_else(|| invalid(TokenDefect::Malformed))?,
            expires_at,
            email: claims.email,
            name: claims.name,
            locale: claims.locale,
            zoneinfo: claims.zoneinfo,
        })
```

- [ ] **Step 8: Run the tests and see them pass**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-703-zitadel/rs
cargo nextest run -p paigasus-iam --lib --locked -E 'test(/validator::tests/)'
```

Expected: every validator test PASSES, the old ones included
(`duplicate_cnf_claim_is_never_authenticated`, `refusal_logs_issuer_and_marker_only`,
`repeated_refusals_log_once_per_issuer_and_defect`).

- [ ] **Step 9: Prove that the tests bite (mutation battery)**

Run each mutation with the Edit tool, run the command, then revert it with the Edit tool (swap
`old_string` and `new_string`). Do NOT use `git checkout --` or `git stash`: they also remove the
uncommitted change under test. Each mutation compiles, so a red is a real assertion failure.

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-703-zitadel/rs
cargo nextest run -p paigasus-iam --lib --locked --no-fail-fast -E 'test(/validator::tests/)'
```

1. `if let Some(name) = claim_marker {` → `if let Some(name) = claim_marker.filter(|_| false) {`.
   Expected FAIL (seven tests): `configured_claim_refusal_logs_issuer_and_claim_name_only`,
   `marker_list_is_per_issuer`, `null_value_is_not_a_marker_and_any_other_value_is`,
   `refresh_grant_tokens_keep_their_kind`, `refuses_zitadel_human_id_token`,
   `refuses_zitadel_machine_id_token`, `wrong_recipe_fails_closed_and_names_the_claim`. Revert.
2. `if members.contains_key(&name) {` → `if false && members.contains_key(&name) {`.
   Expected FAIL: `duplicate_member_is_malformed_on_the_strict_path`. Revert.
3. `.is_some_and(|value| !value.is_null())` → `.is_some()`.
   Expected FAIL: `null_value_is_not_a_marker_and_any_other_value_is`. Revert.

After the three reverts, run the command again. Expected: every validator test PASSES. Then run
`git diff --stat` and confirm that only `validator.rs` changed in this task.

- [ ] **Step 10: Run the whole lib suite, fmt and clippy**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-703-zitadel/rs
cargo nextest run -p paigasus-iam --lib --locked
cargo fmt --check -p paigasus-iam
cargo clippy -p paigasus-iam --all-targets --locked -- -D warnings
```

Expected: all lib tests PASS; fmt prints nothing; clippy exits 0.

- [ ] **Step 11: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-703-zitadel
git add rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs
git commit -m "feat(rs): IAM refuses a token by a configured ID-token marker claim (SMA-703)" \
  -m "An issuer with id_token_marker_claims decodes the verified payload as a strict map that
refuses a repeated top-level member, and refuses a token that carries a configured name with a
value other than null, after the SMA-686 markers. The refusal logs the marker claim <name>. An
issuer with an empty list keeps the old decode. IAM logs the configured names once at boot. The
tests use the measured Zitadel v4.15.3 token shapes." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git log --oneline -1
```

---

### Task 3: The chart value `oidc.idTokenMarkerClaims`

**Files:**
- Modify: `charts/paigasus/values.yaml:166-171` (new key after `acknowledgeClientIdAudience`)
- Modify: `charts/paigasus/templates/_iam-backend.tpl:41-103` (doc, call, two new helpers at the end)
- Modify: `charts/paigasus/templates/backend-deployment.yaml:120-124` (comment line and `IAM_AUTHN__ISSUERS`)
- Modify: `charts/paigasus/README.md:236-238` (new section before "The console authorization request")
- Modify: `charts/paigasus/tests/env.sh:687-695` (M rows before the final verdict)
- Modify: `charts/paigasus/tests/refusals.sh:268-269` (rows before `expect_render "iam only"`)

**Interfaces:**
- Consumes: the env form `id_token_marker_claims=["…",…]` that Task 1's figment test parses.
- Produces: Helm helpers `paigasus.iamIdTokenMarkerClaims` (returns `,id_token_marker_claims=[…]` or `""`) and `paigasus.validateIdTokenMarkerClaims` (fails the render); the value `oidc.idTokenMarkerClaims` (default `[]`); the YAML comment line `            # oidc.idTokenMarkerClaims is set: IAM refuses a token that carries one of these claims.`

- [ ] **Step 1: Write the failing `env.sh` rows**

In `charts/paigasus/tests/env.sh`, Edit `old_string`:

```bash
if [ "$ec" -eq 0 ]; then echo "== chart env OK =="; fi
exit "$ec"
```

`new_string`:

```bash
# oidc.idTokenMarkerClaims (SMA-703). Renders go to a file, as for check_audience. One row per
# property:
#   M1 default      IAM_AUTHN__ISSUERS is the value from before SMA-703, and the
#                   "oidc.idTokenMarkerClaims is set" comment line is absent.
#   M2 reuse-values-no-key
#                   `--set oidc.idTokenMarkerClaims=null`. The template reads nil. The value does
#                   not change (spec T20).
#   M3 set          The Zitadel recipe. Each name is quoted with %q, in values order, after the
#                   audiences. config.rs parses this exact form in
#                   issuers_env_in_the_chart_form_parses_id_token_marker_claims.
#   M4 empty-list-in-file
#                   `idTokenMarkerClaims: []` in a values file renders no key. This is a rollback
#                   form. `--set oidc.idTokenMarkerClaims={}` is NOT one: helm makes it [""], and
#                   refusals.sh refuses it.
#   M5 with-audience
#                   Runbook option 1: oidc.audience is a Zitadel project id of digits only. The
#                   audience stays a quoted string, and the names follow it.
#   M6 restart-scope
#                   A change of the value changes the IAM pod template and no console pod template.
# A fifth row counter reds the script when an M row call line is deleted.
MARKER_ROWS=0
MARKER_ROWS_WANT=6

# check_markers <label> <want suffix or -> <want audience> <present|absent> [helm args...]
# The fourth argument is the state of the "oidc.idTokenMarkerClaims is set" YAML comment line.
check_markers() {
  local label="$1" suffix="$2" audience="$3" comment="$4"; shift 4
  local out
  MARKER_ROWS=$((MARKER_ROWS + 1))
  if ! helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" "$@" >"$TMP/markers.yaml" 2>"$TMP/markers.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/markers.err"; ec=1; return 0
  fi
  if ! out="$(SUFFIX="$suffix" AUDIENCE="$audience" COMMENT="$comment" python3 -c '
import os, sys, yaml
suffix = "" if os.environ["SUFFIX"] == "-" else os.environ["SUFFIX"]
want = "[{issuer=\"https://idp.example.test/realms/paigasus\",audiences=[\"" + os.environ["AUDIENCE"] + "\"]" + suffix + "}]"
line = "            # oidc.idTokenMarkerClaims is set: IAM refuses a token that carries one of these claims."
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
    problems.append("the oidc.idTokenMarkerClaims comment line renders " + str(count) + " time(s), want 1")
if os.environ["COMMENT"] == "absent" and count != 0:
    problems.append("the oidc.idTokenMarkerClaims comment line renders " + str(count) + " time(s), want 0")
print("|".join(problems) if problems else "OK")' "$TMP/markers.yaml" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

# check_markers_restart <label>: the value changes the IAM pod template only. It reuses
# check_boot_restart, and moves the row from the B counter to the M counter.
check_markers_restart() {
  local label="$1"
  MARKER_ROWS=$((MARKER_ROWS + 1))
  BOOT_ROWS=$((BOOT_ROWS - 1))
  check_boot_restart "$label" --set 'oidc.idTokenMarkerClaims={at_hash,azp}'
}

printf 'oidc:\n  idTokenMarkerClaims: []\n' >"$TMP/markers-empty.yaml"
printf 'oidc:\n  audience: "393381921683406851"\n  idTokenMarkerClaims: ["at_hash", "azp"]\n' >"$TMP/markers-zitadel.yaml"
ZITADEL_SUFFIX=',id_token_marker_claims=["at_hash","azp"]'

check_markers "M1 default"               -                 paigasus-console   absent
check_markers "M2 reuse-values-no-key"   -                 paigasus-console   absent  --set oidc.idTokenMarkerClaims=null
check_markers "M3 set"                   "$ZITADEL_SUFFIX" paigasus-console   present --set 'oidc.idTokenMarkerClaims={at_hash,azp}'
check_markers "M4 empty-list-in-file"    -                 paigasus-console   absent  -f "$TMP/markers-empty.yaml"
check_markers "M5 with-audience"         "$ZITADEL_SUFFIX" 393381921683406851 present -f "$TMP/markers-zitadel.yaml"
check_markers_restart "M6 restart-scope"

if [ "$MARKER_ROWS" -lt "$MARKER_ROWS_WANT" ]; then
  echo "FAIL [marker rows]: $MARKER_ROWS marker row(s) ran, want $MARKER_ROWS_WANT"; ec=1
fi

if [ "$ec" -eq 0 ]; then echo "== chart env OK =="; fi
exit "$ec"
```

- [ ] **Step 2: Write the failing `refusals.sh` rows**

In `charts/paigasus/tests/refusals.sh`, Edit `old_string`:

```bash
expect_render "iam only" --set zones.gateway.enabled=false
```

`new_string`:

```bash
# SMA-703 (spec D5). oidc.idTokenMarkerClaims copies the IamConfig::validate rules, because a boot
# failure stops the one IAM replica. Each needle carries the key path and the index.
MARKERS=oidc.idTokenMarkerClaims
expect_fail "markers not a list" "oidc.idTokenMarkerClaims must be a list" \
  --set "$MARKERS=at_hash"
expect_fail "markers item a number" "oidc.idTokenMarkerClaims[0] must be a string" \
  --set "$MARKERS={123}"
# Review Focus 1. `--set x={}` does not clear a list: helm 3.22.0 makes it [""].
expect_fail "markers set to {}" "oidc.idTokenMarkerClaims[0] is empty" \
  --set "$MARKERS={}"
# A space after the comma in --set gives the item " azp".
expect_fail "markers item with a space" "oidc.idTokenMarkerClaims[1] is \" azp\"" \
  --set "$MARKERS={at_hash, azp}"
expect_fail "markers item with a tab" "oidc.idTokenMarkerClaims[0] is \"at_hash\\t\"" \
  --set-string "$MARKERS[0]=$(printf 'at_hash\t')"
# A control character: Go's %q writes \x01, which figment cannot read.
expect_fail "markers item with a control character" "oidc.idTokenMarkerClaims[0] is \"a\\x01b\"" \
  --set-string "$MARKERS[0]=$(printf 'a\001b')"
expect_fail "markers item with a quote" "oidc.idTokenMarkerClaims[0] is \"a\\\"b\"" \
  --set-string "$MARKERS[0]=a\"b"
# A name that is not ASCII. The source of this script stays ASCII, so the needle is a prefix.
expect_fail "markers item not ASCII" "oidc.idTokenMarkerClaims[0] is \"azp" \
  --set-string "$MARKERS[0]=$(printf 'azp\303\251')"
expect_fail "markers reserved name" "oidc.idTokenMarkerClaims[1] is \"sub\": every access token carries" \
  --set "$MARKERS={azp,sub}"
expect_fail "markers duplicate" "oidc.idTokenMarkerClaims[2] \"at_hash\" is already in oidc.idTokenMarkerClaims[0]" \
  --set "$MARKERS={at_hash,azp,at_hash}"
expect_render "markers Zitadel recipe" \
  --set "$MARKERS={at_hash,azp}"

expect_render "iam only" --set zones.gateway.enabled=false
```

- [ ] **Step 3: Run the rows and see them fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-703-zitadel
/bin/bash charts/paigasus/tests/env.sh --set ingress.host=console.example.test; echo "rc=$?"
/bin/bash charts/paigasus/tests/refusals.sh --set ingress.host=console.example.test; echo "rc=$?"
```

Expected: `env.sh` rc=1 with `FAIL [M3 set]`, `FAIL [M5 with-audience]` (the value has no
`id_token_marker_claims`, and the comment line renders 0 times) and `FAIL [M6 restart-scope]`
(`iam-backend: spec.template is equal`); M1, M2 and M4 are `ok`. `refusals.sh` rc=1 with
`FAIL [markers …]: rendered, expected a refusal` for all ten `expect_fail` rows;
`markers Zitadel recipe` is `ok`.

- [ ] **Step 4: Add the value to `values.yaml`**

In `charts/paigasus/values.yaml`, the `acknowledgeClientIdAudience` block (`:166-171`) ends with
two comment lines. Edit `old_string` (these two lines):

```yaml
                                    # change of oidc.clientId shows the warning again. Quote the
                                    # value in a values file. See RUNBOOK-chart.md § 6.
```

`new_string`:

```yaml
                                    # change of oidc.clientId shows the warning again. Quote the
                                    # value in a values file. See RUNBOOK-chart.md § 6.
  idTokenMarkerClaims: []   # NOT required. Claim names that the IdP puts into its ID token and
                            # never into its access token (SMA-703). IAM refuses a token that
                            # carries one of them, with any value except null. Empty: no such
                            # check. Zitadel: ["at_hash", "azp"]. Do not name a claim that your
                            # IdP's access token carries: IAM then refuses every token. Each name
                            # is printable ASCII with no space, " or \, and not iss, sub, aud or
                            # exp. A change restarts IAM. To remove it, use [] in a values file
                            # or --set oidc.idTokenMarkerClaims=null, not {}. See
                            # RUNBOOK-chart.md § 6.
```

- [ ] **Step 5: Add the two helpers and the validation call to `_iam-backend.tpl`**

In `charts/paigasus/templates/_iam-backend.tpl`, Edit `old_string`:

```
paigasus.validateIamBackend: the refusals for the two values. paigasus.validate calls it.
```

`new_string`:

```
paigasus.validateIamBackend: the refusals for the two values, and for oidc.idTokenMarkerClaims
(SMA-703, paigasus.validateIdTokenMarkerClaims below). paigasus.validate calls it.
```

Edit `old_string` (the end of `paigasus.validateIamBackend`):

```
{{- $_ := set $seen $e.name $i -}}
{{- end -}}
{{- end -}}
```

`new_string`:

```
{{- $_ := set $seen $e.name $i -}}
{{- end -}}
{{- include "paigasus.validateIdTokenMarkerClaims" . -}}
{{- end -}}

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
{{- fail (printf "oidc.idTokenMarkerClaims[%d] is empty. IamConfig::validate refuses an empty name, and IAM does not boot. To remove the value, use [] in a values file or --set oidc.idTokenMarkerClaims=null, not {}" $i) -}}
{{- end -}}
{{- if not (regexMatch `^[!#-\[\]-~]+$` $n) -}}
{{- fail (printf "oidc.idTokenMarkerClaims[%d] is %q: use printable ASCII only, with no space, no \" and no \\. IAM cannot read another character from IAM_AUTHN__ISSUERS" $i $n) -}}
{{- end -}}
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

- [ ] **Step 6: Render the suffix in `backend-deployment.yaml`**

In `charts/paigasus/templates/backend-deployment.yaml`, Edit `old_string`:

```
{{- if $root.Values.oidc.audience }}
            # oidc.audience is set: IAM accepts that audience, not the client id.
{{- end }}
            - name: IAM_AUTHN__ISSUERS
              value: {{ printf "[{issuer=%q,audiences=[%q]}]" $root.Values.oidc.issuer (include "paigasus.iamAudience" $root) | quote }}
```

`new_string`:

```
{{- if $root.Values.oidc.audience }}
            # oidc.audience is set: IAM accepts that audience, not the client id.
{{- end }}
{{- /*
  oidc.idTokenMarkerClaims (SMA-703). paigasus.iamIdTokenMarkerClaims returns "" for the default
  [], so the default render stays byte-identical. The comment line is inside the `if`, so it does
  not render by default either (charts/CLAUDE.md, golden files).
*/}}
{{- if include "paigasus.iamIdTokenMarkerClaims" $root }}
            # oidc.idTokenMarkerClaims is set: IAM refuses a token that carries one of these claims.
{{- end }}
            - name: IAM_AUTHN__ISSUERS
              value: {{ printf "[{issuer=%q,audiences=[%q]%s}]" $root.Values.oidc.issuer (include "paigasus.iamAudience" $root) (include "paigasus.iamIdTokenMarkerClaims" $root) | quote }}
```

- [ ] **Step 7: Run the chart scripts and see them pass, goldens unchanged**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-703-zitadel
for s in render refusals env maps ingress names ca-bundle; do
  /bin/bash "charts/paigasus/tests/$s.sh" --set ingress.host=console.example.test >"${TMPDIR:-/tmp}/sma703-$s.txt" 2>&1
  echo "$s rc=$?"; grep -E "FAIL|OK ==" "${TMPDIR:-/tmp}/sma703-$s.txt"
done
git diff --stat -- charts/paigasus/tests/golden
```

Expected: every script rc=0 and prints its `== … OK ==` line, with no `FAIL`; `env.sh` shows
`ok [M1 default]` to `ok [M6 restart-scope]`; `refusals.sh` shows the ten `markers …` refusals
`ok`. `git diff --stat -- charts/paigasus/tests/golden` prints nothing.

- [ ] **Step 8: Run the full helm-render gate**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-703-zitadel
moon run repo:helm-render
```

Expected: PASS. If it stops at about 0% CPU on this Mac, the host is in the 512-byte-pipe state
(root `CLAUDE.md`, "This development Mac only"). Then run `/bin/bash ci/helm-render/run.sh` and
read its verdict instead. Do not edit `ci/helm-render/`.

- [ ] **Step 9: Document the value in the chart README**

In `charts/paigasus/README.md`, Edit `old_string`:

```
line is deleted. See `docs/ops/RUNBOOK-chart.md` § 6 for the recommended setup and the migration
order.

## The console authorization request (`oidc.scopes`, `oidc.authorizationAudience`)
```

`new_string`:

```
line is deleted. See `docs/ops/RUNBOOK-chart.md` § 6 for the recommended setup and the migration
order.

## The ID-token marker claims (`oidc.idTokenMarkerClaims`)

IAM can refuse a token that carries a claim which the IdP puts into its ID token only (SMA-703).
Zitadel needs this, because no audience setting separates its two tokens. The chart renders the
list into the one issuer entry of `IAM_AUTHN__ISSUERS` in `templates/backend-deployment.yaml`.

- **Empty or absent (the default).** The chart adds nothing. The render is byte-identical to a
  chart without the value. Under `helm upgrade --reuse-values` from an older release, the key is
  absent. The template then reads nil. The result is the same.
- **Set.** The entry gets `,id_token_marker_claims=["at_hash","azp"]` after `audiences`. Each name
  is quoted with `%q`. One more YAML comment line renders above `IAM_AUTHN__ISSUERS`:
  `# oidc.idTokenMarkerClaims is set: IAM refuses a token that carries one of these claims.`
- **The render fails** when the value is not a list, when a name is not a string or is empty,
  when a name has a character outside printable ASCII, a space, `"` or `\`, when a name is `iss`,
  `sub`, `aud` or `exp`, and when a name occurs twice. IAM refuses these values at boot, and the
  IAM Deployment has one replica with `maxSurge: 0`, so the chart refuses them first. The checks
  are in `paigasus.validateIdTokenMarkerClaims` in `templates/_iam-backend.tpl`. They are not in
  `_helpers.tpl`, so its fixture copies do not change.
- **To remove the value,** use `[]` in a values file, or `--set oidc.idTokenMarkerClaims=null`.
  Do not use `--set oidc.idTokenMarkerClaims={}`: Helm makes it a list with one empty name, and
  the render fails.
- A change of the value restarts the IAM pod and no console pod.

`tests/env.sh` holds the rows `M1 default`, `M2 reuse-values-no-key`, `M3 set`,
`M4 empty-list-in-file`, `M5 with-audience` and `M6 restart-scope`. A fifth row counter checks
them. `tests/refusals.sh` holds one row for each refusal and one valid render. See
`docs/ops/RUNBOOK-chart.md` § 6 for the IdP setup.

## The console authorization request (`oidc.scopes`, `oidc.authorizationAudience`)
```

- [ ] **Step 10: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-703-zitadel
git add charts/paigasus/values.yaml charts/paigasus/templates/_iam-backend.tpl \
  charts/paigasus/templates/backend-deployment.yaml charts/paigasus/README.md \
  charts/paigasus/tests/env.sh charts/paigasus/tests/refusals.sh
git commit -m "feat(repo): chart value oidc.idTokenMarkerClaims for the IAM issuer (SMA-703)" \
  -m "The chart renders the list into the one IAM_AUTHN__ISSUERS entry only when it is not empty,
so the default render and the golden files do not change. The render fails on each value that
IamConfig::validate refuses at boot, and on a name that Go's %q would escape. env.sh gets rows
M1 to M6, and refusals.sh one row for each refusal." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git log --oneline -1
```

---

### Task 4: Runbook § 6 states the Zitadel status

**Files:**
- Modify: `docs/ops/RUNBOOK-chart.md:35` (§ 1 values table), `:136` (§ 5 restart table), `:194-197` (SMA-686 paragraph), `:301-302` (acknowledgement), `:314-315` (per-IdP header), `:350-352` (Dex bullet, then the new Zitadel bullet)

**Interfaces:**
- Consumes: the value name `oidc.idTokenMarkerClaims` (Task 3), the log marker `claim <name>` and the boot line (Task 2), the `env.sh` row name `M6` (Task 3).
- Produces: documentation only.

- [ ] **Step 1: Write the failing text check**

Save nothing to the repo. Run this check; it fails before the edit:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-703-zitadel
python3 -c '
import sys
text = open("docs/ops/RUNBOOK-chart.md").read()
need = [
    "| `oidc.idTokenMarkerClaims` | no |",
    "| `oidc.idTokenMarkerClaims` | the IAM pod, not the consoles |",
    "**IAM refuses a token by a claim name that you configure (SMA-703).**",
    "or a token with a claim named in `oidc.idTokenMarkerClaims` (SMA-703)",
    "- **Zitadel. Measured, Zitadel v4.15.3 with Login v1, 2026-10-02 (SMA-703).**",
    "Set `oidc.idTokenMarkerClaims: [\"at_hash\", \"azp\"]`.",
    "`oidc.idTokenMarkerClaims: [\"c_hash\"]` refuses a Dex ID token from the code flow only.",
    "Do not use `--set oidc.idTokenMarkerClaims={}`",
    "`claim at_hash`",
]
missing = [n for n in need if n not in text]
print("missing:", missing) if missing else print("OK")
sys.exit(1 if missing else 0)'; echo "rc=$?"
```

Expected: rc=1, with all nine strings listed as missing.

- [ ] **Step 2: Add the values-table row (§ 1)**

In `docs/ops/RUNBOOK-chart.md`, Edit `old_string`:

```
| `oidc.acknowledgeClientIdAudience` | no | Set it to the value of `oidc.clientId` to remove the audience warning (§ 6). It does not change what IAM accepts |
```

`new_string`:

```
| `oidc.acknowledgeClientIdAudience` | no | Set it to the value of `oidc.clientId` to remove the audience warning (§ 6). It does not change what IAM accepts |
| `oidc.idTokenMarkerClaims` | no | Claim names that the IdP puts into its ID token and never into its access token. IAM refuses a token that carries one of them. Default `[]`: no such check. Zitadel: `["at_hash", "azp"]` (§ 6) |
```

- [ ] **Step 3: Add the restart-table row (§ 5)**

Edit `old_string`:

```
| `oidc.acknowledgeClientIdAudience` | nothing | it changes only the IAM Deployment's `metadata` annotation and the NOTES, not a pod template (`tests/env.sh` row W14) |
```

`new_string`:

```
| `oidc.acknowledgeClientIdAudience` | nothing | it changes only the IAM Deployment's `metadata` annotation and the NOTES, not a pod template (`tests/env.sh` row W14) |
| `oidc.idTokenMarkerClaims` | the IAM pod, not the consoles | it changes `IAM_AUTHN__ISSUERS` in the IAM pod template (`tests/env.sh` row M6). IAM is not available during the restart, as for `oidc.audience` |
```

- [ ] **Step 4: Add the opt-in to the SMA-686 paragraph (§ 6)**

Edit `old_string`:

```
The check does not protect an IdP whose ID token has no `typ` claim. For such an IdP, a
dedicated API audience is the only protection. This works only when the IdP can put a different
audience into the access token than into the ID token. Dex cannot: both tokens have the same
`aud`, so IAM accepts a Dex ID token as a bearer token. See "Dex" below.
```

`new_string`:

```
The check does not protect an IdP whose ID token has no `typ` claim. For such an IdP, two
protections are possible. A dedicated API audience works only when the IdP can put a different
audience into the access token than into the ID token. Dex and Zitadel cannot: see "Dex" and
"Zitadel" below. The second protection is `oidc.idTokenMarkerClaims`. It works only for an IdP
whose ID token has a claim that its access token never has.

**IAM refuses a token by a claim name that you configure (SMA-703).** Set
`oidc.idTokenMarkerClaims` to a list of claim names. IAM then refuses a token that carries one of
these claims, with any value except `null`. The default is `[]`, and IAM then does no such check.

- Use a name only when the IdP puts it into its ID token and never into its access token. If the
  access token also has the claim, IAM refuses every token of the IdP. Dex puts `at_hash` and
  `nonce` into its access token. Keycloak puts `azp` into its access token.
- Before you set the value, decode one access token for each grant type in use. No access token
  can have a configured claim. Decode one ID token. It must have the claims.
- At start, IAM writes one `info` line with the issuer and the configured names. If this line is
  not in the IAM log, IAM does not use the setting. A wrong key in a raw `iam.toml` gives no other
  sign.
- The IAM log shows a refusal at `info`, with the issuer and the marker `claim <name>`, for
  example `claim at_hash`. The line does not show the claim value. The rate limit above applies.
- The chart refuses a name that is empty, a name with a character outside printable ASCII, a
  space, `"` or `\`, the names `iss`, `sub`, `aud` and `exp`, and a name that occurs two times.
- A change of the value restarts IAM (§ 5).
- To remove the setting, delete the key from your values file, set it to `[]` in a values file,
  or use `--set oidc.idTokenMarkerClaims=null`. Do not use `--set oidc.idTokenMarkerClaims={}`:
  Helm makes it a list with one empty name, and the chart refuses it.
```

- [ ] **Step 5: Extend the acknowledgement sentence**

Edit `old_string`:

```
show. The acknowledgement does not change what IAM accepts. IAM still accepts an ID token as a
bearer token, except a Keycloak ID token (SMA-686).
```

`new_string`:

```
show. The acknowledgement does not change what IAM accepts. IAM still accepts an ID token as a
bearer token, except a Keycloak ID token (SMA-686) or a token with a claim named in
`oidc.idTokenMarkerClaims` (SMA-703).
```

- [ ] **Step 6: Label the per-IdP header, extend Dex, and add the Zitadel bullet**

Edit `old_string`:

```
**Per IdP. Not measured.** These lines state what each IdP offers. This chart did not measure
them.
```

`new_string`:

```
**Per IdP. Not measured.** These lines state what each IdP offers. This chart did not measure
them. The Zitadel item is the one exception: it is measured.
```

Edit `old_string`:

```
- **Dex.** Dex gives the ID token and the access token the same `aud`. No audience setting helps.
  Set `oidc.acknowledgeClientIdAudience` to remove the warning. IAM still accepts a Dex ID token
  as a bearer token. SMA-686 residual R1 stays open for Dex.
```

`new_string`:

```
- **Dex.** Dex gives the ID token and the access token the same `aud`. No audience setting helps.
  Set `oidc.acknowledgeClientIdAudience` to remove the warning. IAM still accepts a Dex ID token
  as a bearer token. `oidc.idTokenMarkerClaims: ["c_hash"]` refuses a Dex ID token from the code
  flow only. A refreshed Dex ID token has no claim that the access token does not also have. So
  SMA-686 residual R1 stays open for Dex (SMA-686 § 2).
- **Zitadel. Measured, Zitadel v4.15.3 with Login v1, 2026-10-02 (SMA-703).**
  - In the human flow, the ID token and the access token have the same `aud`. In the machine flow
    (client credentials), the ID token `aud` contains the access token `aud`. Both tokens have
    `client_id`. No `urn:zitadel:iam:org:project:id:<id>:aud` scope puts an audience into the
    access token only. So `oidc.audience` alone does not refuse a Zitadel ID token.
  - Set the access token type to JWT on the app, and on each machine user. IAM cannot validate
    an opaque access token. An app that you make without a token type gets the opaque type
    (inferred, not measured).
  - IAM needs `email` in the access token (item 2). Zitadel does not put it there, also with
    "User Info inside ID Token" on. Add it with a Zitadel Action. Not measured. Never send the
    ID token instead.
  - Set `oidc.idTokenMarkerClaims: ["at_hash", "azp"]`. Every measured Zitadel ID token has both
    claims. No measured Zitadel access token has one of them.
  - The audience. Option 1, for an install with machine clients: set `oidc.audience` to the
    project id. Each machine client must request the scope
    `urn:zitadel:iam:org:project:id:<project id>:aud`. Without this scope, a machine token has
    only its own client id as `aud`, and IAM refuses it as an audience mismatch. Option 2, for an
    install with the console only: keep the client id as the audience, and set
    `oidc.acknowledgeClientIdAudience`.
  - Before the switch: decode one access token for each grant type in use (authorization code,
    refresh token, client credentials). No access token can have `at_hash` or `azp`. Decode one
    ID token. It must have both claims.
  - After the switch: send an ID token to IAM. Expect a `401` and the IAM log line with
    `claim at_hash`.
  - A change of the value restarts IAM (§ 5).
  - After each Zitadel upgrade, decode the tokens again. If a new version puts `azp` or `at_hash`
    into the access token, IAM refuses every token, and the log line names the claim. If a new
    version removes both claims from the ID token, the protection stops, and nothing warns.
```

- [ ] **Step 7: Run the text check and see it pass**

Run the command of Step 1 again.

Expected: `OK`, rc=0.

- [ ] **Step 8: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-703-zitadel
git add docs/ops/RUNBOOK-chart.md
git commit -m "docs(repo): runbook states the measured Zitadel status and the marker claims (SMA-703)" \
  -m "Section 6 gets the opt-in oidc.idTokenMarkerClaims, the Zitadel v4.15.3 measurement with the
recipe at_hash and azp, the audience options, the checks before and after the switch, and the
partial Dex case with c_hash. Sections 1 and 5 get the value and its IAM restart." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git log --oneline -1
```

---

## Final verification (after Task 4)

- [ ] Run the Rust and chart checks once more on the final tree:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-703-zitadel/rs
cargo nextest run -p paigasus-iam --lib --locked
cargo fmt --check -p paigasus-iam
cargo clippy -p paigasus-iam --all-targets --locked -- -D warnings
cd ..
moon run repo:helm-render
git log --oneline main..HEAD
git status --short
```

Expected: all pass; four new commits on top of the two spec commits; `git status --short` shows
only the spec file that was already modified before this plan, and this plan file if it is not
committed yet.

## Spec coverage

| Spec item | Task |
|---|---|
| D1 field, empty default | Task 1 (field), Task 2 (`ConfiguredIssuer`) |
| D2 boot rules, exact compare | Task 1 (`validate`, T16, T17) |
| D2 boot `info` line | Task 2 (`new`, T15) |
| D3 check after SMA-686 markers, null not a marker, `StrictPayload`, plain path unchanged | Task 2 |
| D4 `claim <name>` log, same message, rate limit | Task 2 (`RefusalDetail::Claim`, T11) |
| D5 chart value, `dig`, `%q`, byte-identical default, `validateIamBackend`, restart | Task 3 |
| D6 runbook § 6 | Task 4 (plus § 1 and § 5 table rows) |
| D7 acceptance mapping | Tasks 2 and 4 |
| D8 no Notion ADR | no task (default kept) |
| T1-T15 | Task 2 (`refuses_zitadel_human_id_token` … `boot_line_names_the_issuer_and_the_marker_claims`) |
| T16, T17 | Task 1 |
| T18 | Task 3, `env.sh` M1-M6 |
| T19 | Task 3, `refusals.sh` markers rows |
| T20 | Task 3, `env.sh` M2 |
| Existing literals | Task 1, Step 5 |
