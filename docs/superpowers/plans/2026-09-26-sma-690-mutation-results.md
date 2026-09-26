# SMA-690 mutation results

This file records two mutation tests. Each mutation checks that a real defect makes the
right tests fail. Each mutation was made in `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs`,
run, and then restored. The commands ran under the `feature/sma-690-iam-sender-constrained-token`
branch, in worktree `.claude/worktrees/sma-690`.

## Mutation A: disable the sender-constraint check

This mutation makes step 7 never trigger. It proves that the tests catch a check that
silently stops firing.

Diff:

```diff
-        if let Some(marker) = sender_constraint_marker(&token_data.claims) {
+        if let Some(marker) = sender_constraint_marker(&token_data.claims).filter(|_| false) {
```

Command: `cd rs && cargo nextest run -p paigasus-iam --lib --no-fail-fast`

Result: the code compiled. 745 tests ran: 741 passed, 4 failed.

RED (failed, as expected):
- `adapters::oidc::validator::tests::refuses_sender_constrained_tokens`
- `adapters::oidc::validator::tests::sender_constrained_refusal_logs_issuer_and_cnf_marker_only`
- `adapters::oidc::validator::tests::dpop_typ_refusal_logs_canonical_marker`
- `adapters::oidc::validator::tests::repeated_sender_constrained_refusals_log_once`

GREEN (passed, as expected): every other test in the 745-test run, including
`accepts_null_cnf`, `accepts_non_marker_typ_values`, `id_token_with_cnf_reports_not_an_access_token`,
`expired_bound_token_reports_expired`, `sender_constraint_marker_names_the_marker`,
`invalid_token_is_401_with_bearer_challenge`,
`every_authn_status_carries_a_registered_reason_and_its_original_message`, and every SMA-686 test.

This result matches the brief.

Command: `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test keycloak_e2e --no-fail-fast`

Result: the test `keycloak_end_to_end_config_only_oidc` failed on both retry attempts (nextest's
normal retry budget). The panic:

```
thread 'keycloak_end_to_end_config_only_oidc' panicked at crates/services/paigasus-iam/tests/keycloak_e2e.rs:232:77:
a DPoP-bound token must not authenticate: AuthnPrincipal { principal_id: PrincipalId(Prn { service: "iam", region: "", org: None, resource_type: "principal", resource_id: ... }), kind: User, status: Active, credential: Oidc { issuer: Issuer("https://127.0.0.1:.../realms/paigasus-test"), subject: "...", expires_at: ... } }
```

This is the `SenderConstrained` `resolve` assertion at line 232. It failed as expected. The test
stopped at this panic. The two 401 assertions that follow this line in the test did not run. This
run does not prove those two assertions. This result matches the brief.

The line was then restored with an edit. `git diff --stat` on the file printed nothing.

## Mutation B: keep the check, drop the refusal log

This mutation keeps the refusal (the token is still rejected), but it stops the refusal-log
line from firing. It proves that the log-content tests catch a missing log call, separately
from the refusal check itself.

Diff:

```diff
-            self.log_refusal(&issuer, TokenDefect::SenderConstrained, RefusalDetail::Binding(marker));
+            let _ = RefusalDetail::Binding(marker);
```

The `let _ = RefusalDetail::Binding(marker);` form was needed. It still constructs the
`Binding` variant, so the code compiles under `warnings = "deny"`. A plain `let _ = marker;`
does not compile, because the `Binding` variant would then never be constructed. That is a
dead-code error under this crate's lint settings.

Command: `cd rs && cargo nextest run -p paigasus-iam --lib --no-fail-fast`

Result: the code compiled. 745 tests ran: 742 passed, 3 failed.

RED (failed, as expected):
- `adapters::oidc::validator::tests::sender_constrained_refusal_logs_issuer_and_cnf_marker_only`
- `adapters::oidc::validator::tests::dpop_typ_refusal_logs_canonical_marker`
- `adapters::oidc::validator::tests::repeated_sender_constrained_refusals_log_once`

GREEN (passed, as expected): `refuses_sender_constrained_tokens` and every other test in the
745-test run.

This result matches the brief.

The line was then restored with an edit.

## Restore check

Command: `git diff --stat -- rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs`

Result: no output. The file matches the Task 1 commit (`4655d6c2`).

Command: `cd rs && cargo nextest run -p paigasus-iam --lib --no-fail-fast && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test keycloak_e2e`

Result: both suites passed. The lib suite: 745 tests run, 745 passed, 0 skipped. The
`keycloak_e2e` suite: 1 test run, 1 passed, 0 skipped.

## Summary

Both mutations compiled. Both mutations turned red exactly the tests the brief named, and
left green exactly the tests the brief named. The restore was verified twice: by an empty
`git diff --stat`, and by a full passing run of both suites.
