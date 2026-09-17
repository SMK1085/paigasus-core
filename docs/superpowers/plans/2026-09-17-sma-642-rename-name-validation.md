# SMA-642 rename name validation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the `rename` path of the three IAM tenancy application services validate the new display name with the same rule `create` uses, and answer `invalid-name`.

**Architecture:** One line is added to each of the three `rename` methods, directly below the line that already parses the slug. It calls `validate_name` and passes the validated (trimmed) name to the repository in place of the raw input. No new abstraction: the name line mirrors the slug line the method already has. Nothing else in the service, the adapters, the proto or the error registry changes, because the `DomainError::InvalidName` -> `"invalid-name"` path already exists end to end.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), `cargo nextest`, `tokio::test`; TypeScript for two comment-only edits.

**Spec:** `docs/superpowers/specs/2026-09-17-sma-642-rename-name-validation-design.md`

## Global Constraints

- Every source file opens with an SPDX header: `// SPDX-License-Identifier: Apache-2.0`. All files in this plan already exist and already carry one. Do not add a second.
- Conventional commits with a workspace scope. The valid scopes are `rs, py, ts, contracts, ci, docs, deps, release, repo, claude, workspace`. This plan uses `fix(rs)`, `test(rs)` and `docs(ts)`.
- Do NOT put a `#NNN` line or a `token: value` line in a commit BODY. commitlint reads it as a footer and fails `footer-leading-blank`.
- Prefix any shell command that needs moon/uv/buf/nextest with:
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`
- `NAME_MAX_CHARS` is **256 Unicode scalar values** (`rs/crates/libs/paigasus-iam-core/src/tenancy.rs:11`). `validate_name` trims first, then bounds. 256 is accepted; 257 is refused.
- `TenancyError::InvalidName` carries a `String` (`rs/crates/services/paigasus-iam/src/application/error.rs:40`). Assert it with `matches!(…, TenancyError::InvalidName(_))`. `assert_eq!` against the bare variant does not compile.
- **Never write the literal string `"invalid-name"` in any file under `rs/crates/services/paigasus-iam/src/`.** None of those three application files is in `ci/error-registry/check.py`'s `MANIFEST`, so a literal there reds `repo:error-code-single-site`. The literal IS allowed in `rs/crates/services/paigasus-iam/tests/http_tenancy.rs`, which is not a `src/` file.
- Run cargo from `rs/`, never from the repository root.
- Do not bypass the git hooks with `--no-verify`.

---

## File Structure

| File | Change |
|---|---|
| `rs/crates/services/paigasus-iam/src/application/organizations.rs` | `rename` validates the name; test module gains a store-aware helper and 8 tests |
| `rs/crates/services/paigasus-iam/src/application/teams.rs` | `rename` validates the name; test module gains 4 tests |
| `rs/crates/services/paigasus-iam/src/application/projects.rs` | `rename` validates the name; test module gains 3 tests |
| `rs/crates/services/paigasus-iam/tests/http_tenancy.rs` | 2 integration cases over the real HTTP router |
| `ts/apps/iam-console/lib/form.ts` | 3 doc comments corrected. No code change. |
| `ts/apps/iam-console/tests/unit/form.test.ts` | 1 comment corrected. No code change. |

Task order matters: Task 1 establishes the pattern and carries the semantics tests, Tasks 2 and 3 repeat the one-line change with their own presence tests, Task 4 proves the wire behaviour, Task 5 fixes the comments the earlier tasks falsify.

---

### Task 1: Organizations — validate the name on rename

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/organizations.rs:302-311` (the `rename` method) and `:22-25` (the `use paigasus_iam_core::{…}` list)
- Test: same file, the `#[cfg(test)] mod tests` block starting at `:385`

**Interfaces:**
- Consumes: `validate_name(input: &str) -> Result<String, DomainError>` from `paigasus_iam_core` (already re-exported at `paigasus-iam-core/src/lib.rs:37`). `From<DomainError> for TenancyError` exists at `application/error.rs:334`, so `?` converts it.
- Produces: the pattern `let name = new_name.map(validate_name).transpose()?;` followed by `name.as_deref()` at the `rename_in` call. Tasks 2 and 3 repeat exactly this.

- [ ] **Step 1: Add the store-aware test helper**

The existing helpers build `InMemoryOrgs::default()` and drop the store, so no test can reach the
stored row. Two of this task's tests must plant a name that `create` would refuse. Add this helper
to the test module, directly after `service_with_fakes()` (which ends at `:421`):

```rust
    /// Builds an `OrganizationService` over a store the CALLER keeps a handle to, so a test can
    /// plant a stored name that `create` would refuse (SMA-642 D4). Every other helper here
    /// builds `InMemoryOrgs::default()` and drops the store, which no test could then reach.
    fn service_over_store(store: TenancyStore, clock: FixedClock) -> OrganizationService<InMemoryOrgs, SeqIds, FixedClock> {
        OrganizationService::new(OrganizationServiceDeps {
            repo: InMemoryOrgs(store),
            uow: Arc::new(FakeUnitOfWork::default()),
            outbox: Arc::new(FakeOutbox::default()),
            audit: Arc::new(FakeAuditLog::default()),
            gen_bumper: Arc::new(CountingGenBumper::default()),
            policy_gen_bumper: Arc::new(FakePolicyGenBumper::default()),
            ids: SeqIds::default(),
            clock,
        })
    }

    /// Overwrites a stored name directly, bypassing every validation path — the only way to
    /// reproduce a row the unvalidated rename wrote before SMA-642 closed it.
    fn plant_stored_name(store: &TenancyStore, id: Uuid, name: &str) {
        store.orgs.lock().unwrap().get_mut(&id).expect("org was created above").name = name.to_owned();
    }
```

Add `TenancyStore` to the test module's `use crate::application::fakes::{…}` list at `:387`.

- [ ] **Step 2: Write the failing tests**

Append these to the test module. They are the § 6.1 organization row plus all of § 6.2 except the
ancestor-archived case (Task 2 owns that one — an organization has no ancestor).

```rust
    /// SMA-642 § 6.1: the three refusals the issue names. Each proves `rename` calls
    /// `validate_name` at all. `matches!`, not `assert_eq!`: the variant carries a String.
    #[tokio::test]
    async fn rename_refuses_an_empty_blank_or_overlong_name() {
        let svc = new_service();
        let id = svc.create(&actor(1), "acme", "Acme").await.unwrap().organization.id.uuid();

        let too_long = "x".repeat(257);
        for bad in ["", "   ", too_long.as_str()] {
            let err = svc.rename(id, None, Some(bad), &actor(2)).await.unwrap_err();
            assert!(matches!(err, TenancyError::InvalidName(_)), "a rename to {bad:?} must be refused, got {err:?}");
        }
    }

    /// SMA-642 § 6.2: the bound is INCLUSIVE at 256 and counts Unicode scalar values, not bytes.
    /// A `>=` in place of `>` fails the first row; a byte count fails the second, because "ü" is
    /// two bytes.
    #[tokio::test]
    async fn rename_accepts_the_upper_bound_in_scalar_values() {
        let svc = new_service();
        let id = svc.create(&actor(1), "acme", "Acme").await.unwrap().organization.id.uuid();

        let max_ascii = "x".repeat(256);
        assert_eq!(svc.rename(id, None, Some(&max_ascii), &actor(2)).await.unwrap().node.name, max_ascii);

        let max_wide = "ü".repeat(256);
        assert_eq!(svc.rename(id, None, Some(&max_wide), &actor(2)).await.unwrap().node.name, max_wide);
    }

    /// SMA-642 D2: `validate_name` returns the TRIMMED name and `create` stores that, so `rename`
    /// must store it too. Storing the raw input would keep the exact defect this issue closes.
    #[tokio::test]
    async fn rename_stores_the_trimmed_name() {
        let svc = new_service();
        let id = svc.create(&actor(1), "acme", "Acme").await.unwrap().organization.id.uuid();

        let renamed = svc.rename(id, None, Some("  Acme Corp.  "), &actor(2)).await.unwrap();
        assert_eq!(renamed.node.name, "Acme Corp.", "the stored name must be trimmed, as create's is");
    }

    /// SMA-642 D2 consequence 1: because the application now trims before the repository's
    /// byte-exact comparison, a name differing only in surrounding whitespace is the SAME name.
    /// The rename must therefore change nothing at all.
    #[tokio::test]
    async fn a_whitespace_only_difference_is_a_no_op() {
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let (svc, outbox, audit, _bumper, _uow) = service_with_fakes_and_clock(clock.clone());
        let id = svc.create(&actor(1), "acme", "Acme").await.unwrap().organization.id.uuid();
        let events_after_create = outbox.0.lock().unwrap().len();
        let entries_after_create = audit.0.lock().unwrap().len();

        clock.set(t0 + Duration::seconds(10));
        let same = svc.rename(id, None, Some("  Acme  "), &actor(2)).await.unwrap();

        assert_eq!(same.node.name, "Acme");
        assert_eq!(same.node.updated_at, t0, "a whitespace-only difference must not advance updated_at");
        assert_eq!(same.node.modified_by.as_ref(), Some(&actor(1)), "a whitespace-only difference must not restamp the modifier");
        assert_eq!(outbox.0.lock().unwrap().len(), events_after_create, "a no-op rename must emit no event");
        assert_eq!(audit.0.lock().unwrap().len(), entries_after_create, "a no-op rename must record no audit entry");
    }

    /// SMA-642 D2 consequence 2, the direction that WRITES. A name stored untrimmed is only
    /// reachable through the unvalidated rename this issue closes. Re-sending it was a no-op
    /// before; now the application trims first, the repository's byte-exact comparison fails, and
    /// the call normalizes the row. That is intended, and it is not silent: it emits the event and
    /// the audit entry that say what changed.
    #[tokio::test]
    async fn a_legacy_untrimmed_name_is_normalized_by_the_next_rename() {
        let store = TenancyStore::default();
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = service_over_store(store.clone(), clock.clone());
        let id = svc.create(&actor(1), "acme", "Acme").await.unwrap().organization.id.uuid();
        plant_stored_name(&store, id, "  Acme  ");

        clock.set(t0 + Duration::seconds(10));
        let renamed = svc.rename(id, None, Some("  Acme  "), &actor(2)).await.unwrap();

        assert_eq!(renamed.node.name, "Acme", "the legacy untrimmed name must be normalized");
        assert_eq!(renamed.node.updated_at, t0 + Duration::seconds(10), "normalizing is a real write and must advance updated_at");
        assert_eq!(renamed.node.modified_by.as_ref(), Some(&actor(2)), "normalizing is a real write and must restamp the modifier");
    }

    /// SMA-642 D3: the name check runs before `uow.begin()`, so it outranks every refusal the
    /// repository raises inside the transaction. Without the check, this call answers NodeArchived.
    #[tokio::test]
    async fn a_bad_name_outranks_an_archived_node() {
        let svc = new_service();
        let id = svc.create(&actor(1), "acme", "Acme").await.unwrap().organization.id.uuid();
        svc.archive(id, &actor(1)).await.unwrap();

        let err = svc.rename(id, None, Some(""), &actor(2)).await.unwrap_err();
        assert!(matches!(err, TenancyError::InvalidName(_)), "invalid-name must outrank node-archived, got {err:?}");
    }

    /// SMA-642 D4: a name IAM already stored that breaks the rule is NOT migrated and does not
    /// block a rename that supplies no name. This is the exact shape the iam-console sends for a
    /// slug-only rename — `renameChange` omits an unchanged name (`ts/apps/iam-console/lib/form.ts:72-79`).
    #[tokio::test]
    async fn a_slug_only_rename_survives_a_legacy_overlong_name() {
        let store = TenancyStore::default();
        let svc = service_over_store(store.clone(), FixedClock::default());
        let id = svc.create(&actor(1), "acme", "Acme").await.unwrap().organization.id.uuid();
        let legacy = "x".repeat(300);
        plant_stored_name(&store, id, &legacy);

        let renamed = svc.rename(id, Some("acme-2"), None, &actor(2)).await.unwrap();
        assert_eq!(renamed.node.slug.as_str(), "acme-2");
        assert_eq!(renamed.node.name, legacy, "a slug-only rename must leave a legacy name untouched");
    }

    /// SMA-642 Risk 3: the whole premise of the issue is that `create` and `rename` must accept
    /// the same set of names. This is the only test that fails if the two rules ever diverge —
    /// the per-file tests above would all still pass.
    #[tokio::test]
    async fn create_and_rename_accept_the_same_names() {
        let cases: Vec<(String, bool)> = vec![
            ("Acme".to_owned(), true),
            ("  Acme  ".to_owned(), true),
            ("ü".repeat(256), true),
            ("x".repeat(256), true),
            (String::new(), false),
            ("   ".to_owned(), false),
            ("x".repeat(257), false),
        ];

        for (name, want_ok) in &cases {
            let name = name.as_str();
            let want_ok = *want_ok;
            let svc = new_service();
            let created = svc.create(&actor(1), "acme", name).await;
            assert_eq!(created.is_ok(), want_ok, "create disagreed on {name:?}");

            let svc = new_service();
            let id = svc.create(&actor(1), "acme", "Seed").await.unwrap().organization.id.uuid();
            let renamed = svc.rename(id, None, Some(name), &actor(2)).await;
            assert_eq!(renamed.is_ok(), want_ok, "rename disagreed on {name:?}");

            if want_ok {
                assert_eq!(created.unwrap().organization.name, renamed.unwrap().node.name, "create and rename stored different names for {name:?}");
            }
        }
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib application::organizations
```

Expected: FAIL. `rename_refuses_an_empty_blank_or_overlong_name`, `a_bad_name_outranks_an_archived_node` and `create_and_rename_accept_the_same_names` fail because `rename` returns `Ok`. `rename_stores_the_trimmed_name` and `a_legacy_untrimmed_name_is_normalized_by_the_next_rename` fail on the stored value. `a_whitespace_only_difference_is_a_no_op` fails because the untrimmed input is treated as a change.

`rename_accepts_the_upper_bound_in_scalar_values` and `a_slug_only_rename_survives_a_legacy_overlong_name` PASS already — they assert behaviour the change must preserve, not add. That is expected and correct.

- [ ] **Step 4: Add `validate_name` to the import list**

In the `use paigasus_iam_core::{…}` statement at `:22-25`, add `validate_name` in alphabetical
position at the end of the list. Let rustfmt reflow the line; do not hand-wrap it.

- [ ] **Step 5: Implement the validation**

In `rename` (`:302`), the current body opens:

```rust
        if new_slug.is_none() && new_name.is_none() {
            return Err(TenancyError::NothingToRename);
        }
        let slug = new_slug.map(Slug::parse).transpose()?;
        let stamp = Stamp::new(self.clock.now(), actor.clone());
```

Add the name line directly below the slug line, so the two fields are symmetric:

```rust
        if new_slug.is_none() && new_name.is_none() {
            return Err(TenancyError::NothingToRename);
        }
        let slug = new_slug.map(Slug::parse).transpose()?;
        // SMA-642: mirrors the slug line above. `create` validates through `Organization::new`;
        // without this, `rename` stores a name `create` refuses. `validate_name` returns the
        // TRIMMED name and that is what gets stored, exactly as `create` stores it (D2).
        let name = new_name.map(validate_name).transpose()?;
        let stamp = Stamp::new(self.clock.now(), actor.clone());
```

Then change the `rename_in` call at `:311` to pass the validated name:

```rust
        let out = self.repo.rename_in(&*tx, id, slug.as_ref(), name.as_deref(), &stamp).await?;
```

- [ ] **Step 6: Update the method's doc comment**

The doc comment at `:293-301` describes the refusals. Add one line to it after the first paragraph:

```rust
    /// The new name is validated with the same rule `create` uses (`validate_name`: trimmed, not
    /// empty, at most `NAME_MAX_CHARS` scalar values) and answers `InvalidName` (SMA-642). The
    /// check runs before the transaction opens, so it outranks every refusal the repository
    /// raises. The stored name is the TRIMMED one.
```

- [ ] **Step 7: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib application::organizations
```

Expected: PASS, every test in the module. The pre-existing tests named in the spec's § 6.4
(`rename_to_identical_values_is_a_no_op`, `rename_with_a_matching_slug_but_a_new_name_still_changes`,
`a_no_op_rename_emits_nothing_but_a_real_one_emits`) must pass **unedited**. If one of them needs an
edit, stop: the change altered behaviour beyond the spec.

- [ ] **Step 8: Check formatting and lints**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt --check && cargo clippy -p paigasus-iam --all-targets -- -D warnings
```

Expected: both clean. If `cargo fmt --check` reports the `use` line, run `cargo fmt` and re-run.

- [ ] **Step 9: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/application/organizations.rs
git commit -m "fix(rs): validate the new name when renaming an organization (SMA-642)

The rename path passed new_name to the repository unchecked, so IAM
stored a name that create refuses. It now calls validate_name, the same
rule create uses, and answers invalid-name.

The stored name is the trimmed one, as create stores it. A name that
differs only by surrounding whitespace is therefore the same name and
the rename changes nothing.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: Teams — validate the name on rename

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/teams.rs:178-187` (the `rename` method) and `:18-20` (the `use paigasus_iam_core::{…}` list)
- Test: same file, the `#[cfg(test)] mod tests` block

**Interfaces:**
- Consumes: the pattern Task 1 produced — `let name = new_name.map(validate_name).transpose()?;` and `name.as_deref()` at the `rename_in` call.
- Produces: nothing new. Task 3 repeats the same pattern.

This task also owns the ancestor-archived case, which an organization cannot express: the org's
in-transaction guard reads its own status alone (`pg_organizations.rs:238`), while a team folds its
org's status too (`pg_teams.rs:200`).

- [ ] **Step 1: Write the failing tests**

Append to the test module. `new_service(store)` (`:286`), `seed_org` (`:324`), `actor` (`:317`) and
`test_stamp` already exist there.

```rust
    /// SMA-642 § 6.1: the three refusals the issue names, on a team. Each proves `TeamService::
    /// rename` calls `validate_name` at all — the per-file half of the check, which no test in
    /// organizations.rs can cover.
    #[tokio::test]
    async fn rename_refuses_an_empty_blank_or_overlong_name() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 9800, "acme", &test_stamp(Utc::now(), 1));
        let svc = new_service(store);
        let id = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap().node.id.uuid();

        let too_long = "x".repeat(257);
        for bad in ["", "   ", too_long.as_str()] {
            let err = svc.rename(id, None, Some(bad), &actor(2)).await.unwrap_err();
            assert!(matches!(err, TenancyError::InvalidName(_)), "a rename to {bad:?} must be refused, got {err:?}");
        }
    }

    /// SMA-642 D3, the case an organization cannot express. A team's in-transaction guard folds
    /// its org's status (`pg_teams.rs:200`), so an active team under an archived org answers
    /// ParentArchived. The name check runs before the transaction opens, so it must win.
    #[tokio::test]
    async fn a_bad_name_outranks_an_ancestor_archived_node() {
        let store = TenancyStore::default();
        let stamp = test_stamp(Utc::now(), 1);
        let org = seed_org(&store, 9801, "acme", &stamp);
        let svc = new_service(store.clone());
        let id = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap().node.id.uuid();

        InMemoryOrgs(store.clone()).set_status(org, NodeStatus::Archived, &stamp).await.unwrap();

        // The guard is real: a VALID name on this team is refused as ParentArchived.
        assert_eq!(svc.rename(id, None, Some("Platform"), &actor(2)).await.unwrap_err(), TenancyError::ParentArchived);

        // And invalid-name still outranks it.
        let err = svc.rename(id, None, Some(""), &actor(2)).await.unwrap_err();
        assert!(matches!(err, TenancyError::InvalidName(_)), "invalid-name must outrank parent-archived, got {err:?}");
    }

    /// SMA-642 D2: the stored name is the trimmed one on a team too. Task 1 proves the semantics
    /// on an organization; this proves THIS file's line passes the validated value down rather
    /// than the raw input — a copy that validates and then forwards `new_name` would pass the
    /// refusal test above and fail here.
    #[tokio::test]
    async fn rename_stores_the_trimmed_name() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 9802, "acme", &test_stamp(Utc::now(), 1));
        let svc = new_service(store);
        let id = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap().node.id.uuid();

        let renamed = svc.rename(id, None, Some("  Platform  "), &actor(2)).await.unwrap();
        assert_eq!(renamed.node.name, "Platform");
    }
```

Add `InMemoryOrgs`, `NodeStatus` and `TenancyStore` to the test module's import lists if they are
not already there. Check the existing `use` lines before adding — `seed_org` and
`team_effective_status_follows_org` (`:346`) already use `InMemoryOrgs` and `NodeStatus`, so they
are almost certainly imported.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib application::teams
```

Expected: FAIL. `rename_refuses_an_empty_blank_or_overlong_name` fails because `rename` returns
`Ok`. `a_bad_name_outranks_an_ancestor_archived_node` fails on its second assertion — the call
answers `ParentArchived` instead of `InvalidName`. `rename_stores_the_trimmed_name` fails on the
stored value.

- [ ] **Step 3: Add `validate_name` to the import list**

In the `use paigasus_iam_core::{…}` statement at `:18-20`, add `validate_name` at the end of the
list. Let rustfmt reflow.

- [ ] **Step 4: Implement the validation**

In `rename` (`:178`), add the name line below the slug line and pass it down:

```rust
        if new_slug.is_none() && new_name.is_none() {
            return Err(TenancyError::NothingToRename);
        }
        let slug = new_slug.map(Slug::parse).transpose()?;
        // SMA-642: mirrors the slug line above. `create` validates through `Team::new`; without
        // this, `rename` stores a name `create` refuses. The stored name is the TRIMMED one (D2).
        let name = new_name.map(validate_name).transpose()?;
        let stamp = Stamp::new(self.clock.now(), actor.clone());
```

And at `:187`:

```rust
        let out = self.repo.rename_in(&*tx, id, slug.as_ref(), name.as_deref(), &stamp).await?;
```

- [ ] **Step 5: Update the method's doc comment**

Add to the doc comment at `:169-177`:

```rust
    /// The new name is validated with the same rule `create` uses (`validate_name`) and answers
    /// `InvalidName` (SMA-642). The check runs before the transaction opens, so it outranks
    /// `NodeArchived` and `ParentArchived`. The stored name is the TRIMMED one.
```

- [ ] **Step 6: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib application::teams
```

Expected: PASS, every test in the module, with no edit to any pre-existing test.

- [ ] **Step 7: Check formatting and lints**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt --check && cargo clippy -p paigasus-iam --all-targets -- -D warnings
```

Expected: both clean.

- [ ] **Step 8: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/application/teams.rs
git commit -m "fix(rs): validate the new name when renaming a team (SMA-642)

Mirrors the organization fix. The rename path now calls validate_name
and answers invalid-name, and it stores the trimmed name.

A team folds its org status inside the transaction, so this file also
proves that an invalid name outranks parent-archived.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: Projects — validate the name on rename

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/projects.rs:194-203` (the `rename` method) and `:18-21` (the `use paigasus_iam_core::{…}` list)
- Test: same file, the `#[cfg(test)] mod tests` block starting at `:284`

**Interfaces:**
- Consumes: the pattern Tasks 1 and 2 produced. Note this service's repository field is `self.projects`, not `self.repo`.
- Produces: nothing new. This is the last of the three.

- [ ] **Step 1: Write the failing tests**

Append to the test module. `new_service(store)` (`:300`), `seed_org_and_team` (`:343`), `actor` and
`test_stamp` already exist there.

```rust
    /// SMA-642 § 6.1: the three refusals the issue names, on a project. Proves
    /// `ProjectService::rename` calls `validate_name` at all.
    #[tokio::test]
    async fn rename_refuses_an_empty_blank_or_overlong_name() {
        let store = TenancyStore::default();
        let (_org, team) = seed_org_and_team(&store, 9900, 9901, &test_stamp(Utc::now(), 1));
        let svc = new_service(store);
        let id = svc.create(team, "api", "API", &actor(1)).await.unwrap().node.id.uuid();

        let too_long = "x".repeat(257);
        for bad in ["", "   ", too_long.as_str()] {
            let err = svc.rename(id, None, Some(bad), &actor(2)).await.unwrap_err();
            assert!(matches!(err, TenancyError::InvalidName(_)), "a rename to {bad:?} must be refused, got {err:?}");
        }
    }

    /// SMA-642 D2: the stored name is the trimmed one. This catches a copy of the fix that
    /// validates and then forwards the RAW `new_name` — that copy passes the refusal test above
    /// and fails this one. `ProjectService` reads `self.projects`, not `self.repo`.
    #[tokio::test]
    async fn rename_stores_the_trimmed_name() {
        let store = TenancyStore::default();
        let (_org, team) = seed_org_and_team(&store, 9902, 9903, &test_stamp(Utc::now(), 1));
        let svc = new_service(store);
        let id = svc.create(team, "api", "API", &actor(1)).await.unwrap().node.id.uuid();

        let renamed = svc.rename(id, None, Some("  Public API  "), &actor(2)).await.unwrap();
        assert_eq!(renamed.node.name, "Public API");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib application::projects
```

Expected: FAIL on both — the first because `rename` returns `Ok`, the second on the stored value.

- [ ] **Step 3: Add `validate_name` to the import list**

In the `use paigasus_iam_core::{…}` statement at `:18-21`, add `validate_name` at the end. Let
rustfmt reflow.

- [ ] **Step 4: Implement the validation**

In `rename` (`:194`):

```rust
        if new_slug.is_none() && new_name.is_none() {
            return Err(TenancyError::NothingToRename);
        }
        let slug = new_slug.map(Slug::parse).transpose()?;
        // SMA-642: mirrors the slug line above. `create` validates through `Project::new`; without
        // this, `rename` stores a name `create` refuses. The stored name is the TRIMMED one (D2).
        let name = new_name.map(validate_name).transpose()?;
        let stamp = Stamp::new(self.clock.now(), actor.clone());
```

And at `:203` — note the field is `self.projects`:

```rust
        let out = self.projects.rename_in(&*tx, id, slug.as_ref(), name.as_deref(), &stamp).await?;
```

- [ ] **Step 5: Update the method's doc comment**

Add to the doc comment at `:185-193`:

```rust
    /// The new name is validated with the same rule `create` uses (`validate_name`) and answers
    /// `InvalidName` (SMA-642). The check runs before the transaction opens, so it outranks
    /// `NodeArchived` and `ParentArchived`. The stored name is the TRIMMED one.
```

- [ ] **Step 6: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib
```

Expected: PASS, the whole library test set — all three services together, with no edit to any
pre-existing test.

- [ ] **Step 7: Check formatting and lints**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt --check && cargo clippy -p paigasus-iam --all-targets -- -D warnings
```

Expected: both clean.

- [ ] **Step 8: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/application/projects.rs
git commit -m "fix(rs): validate the new name when renaming a project (SMA-642)

The last of the three tenancy services. The rename path now calls
validate_name and answers invalid-name, and it stores the trimmed name.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: Prove `invalid-name` on the wire

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/http_tenancy.rs`

**Interfaces:**
- Consumes: the three services' validation from Tasks 1-3.
- Produces: nothing other tasks use.

Nothing today asserts `invalid-name` as a response on either transport. This is the only
execution-level proof of the spec's F4 (the wire path) and D3 (the transport-level refusal order).

**This suite needs Docker.** It starts an ephemeral Postgres via `support::start_migrated_postgres()`.

- [ ] **Step 1: Write the failing test**

Add this test function beside `org_lifecycle_over_http` (which ends around `:90`). It follows that
test's own shape exactly: `send(&app, METHOD, uri, body, token)` returns `(StatusCode, Value)`, and
the error code is at `err["error"]["code"]` — the same accessor the `nothing-to-rename` assertion
at `:57-60` and the `slug-conflict` assertion at `:47-49` already use.

```rust
/// SMA-642: the only execution-level proof that a refused name reaches a client as
/// `invalid-name`, and that it does so with the ordering the spec's D3 claims. The three
/// application services' unit tests prove the rule; nothing but this proves the wire.
///
/// The literal code string is safe HERE and nowhere in `src/`: `ci/error-registry/check.py`'s
/// MANIFEST does not list the three application service files, so a literal in one of those
/// reds `repo:error-code-single-site`. A test file is not a `src/` file.
#[tokio::test]
async fn rename_with_a_bad_name_answers_invalid_name_over_http() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("name-guard-user", Some("name-guard@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &token).await;

    let (status, created) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": "namecheck", "name": "Name Check"})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED);
    let org_prn = created["organization"]["prn"].as_str().expect("organization.prn");
    let org_id = org_prn.rsplit('/').next().unwrap();

    // An empty name -> 400 `invalid-name`. Before SMA-642 this was a 200 that stored "".
    let (status, err) = send(&app, "PATCH", &format!("/v1/organizations/{org_id}"), Some(json!({"name": ""})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(err["error"]["code"], "invalid-name");

    // A 257-character name -> the same. `NAME_MAX_CHARS` is 256 and the bound is inclusive.
    let too_long = "x".repeat(257);
    let (status, err) = send(&app, "PATCH", &format!("/v1/organizations/{org_id}"), Some(json!({"name": too_long})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(err["error"]["code"], "invalid-name");

    // The stored name is the TRIMMED one (D2), and a valid name still renames.
    let (status, renamed) = send(&app, "PATCH", &format!("/v1/organizations/{org_id}"), Some(json!({"name": "  Trimmed Name  "})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["name"], "Trimmed Name");

    // D3/F7: `not-found` PRECEDES `invalid-name` on the wire. Under the default
    // `enforce_tenancy = true` the handler runs `get(id)` then `authorize.check` BEFORE it calls
    // `rename` (`adapters/http/organizations.rs:84-87`), so an unknown id answers 404 even with a
    // name the service would refuse. A reader who takes the application-layer order as the wire
    // order gets this backwards.
    let unknown = Uuid::new_v4();
    let (status, err) = send(&app, "PATCH", &format!("/v1/organizations/{unknown}"), Some(json!({"name": ""})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{err}");
    assert_eq!(err["error"]["code"], "not-found");
}
```

`Uuid` is already imported at `:16`; `StatusCode`, `json!`, `send`, `app_with_state` and
`provision_platform_admin` at `:11-15`. No new imports are needed.

- [ ] **Step 2: Run the test to verify assertion 1 fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test http_tenancy
```

Expected: this is run AFTER Tasks 1-3, so assertion 1 should PASS. Verify it genuinely bites by
temporarily reverting Task 1's one-line change, re-running, and confirming the test fails with a
200 instead of a 400. Restore the line afterwards **by re-applying the edit, not by
`git checkout --`** — a checkout would also discard this task's new test file content.

Assertion 2 should pass immediately; it asserts pre-existing ordering.

- [ ] **Step 3: Run the whole suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test http_tenancy
```

Expected: PASS, including the pre-existing `org_lifecycle_over_http`.

If Docker is unreachable the run reds on `docker_preflight`. That is the intended signal, not a
failure of this change — start Docker and re-run. Do NOT set `PAIGASUS_SKIP_DOCKER=1` to get a
green: a `moon run` that greened under it leaves a cached PASS that replays later.

- [ ] **Step 4: Check formatting and lints**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt --check && cargo clippy -p paigasus-iam --all-targets -- -D warnings
```

Expected: both clean.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/http_tenancy.rs
git commit -m "test(rs): prove invalid-name reaches an HTTP client on rename (SMA-642)

Nothing asserted invalid-name as a response on either transport. The
first case pins the reason and the 400; the second pins the order, since
the handler reads and authorizes the node before it calls rename, so an
unknown id answers not-found even with a name the service would refuse.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5: Correct the iam-console comments this change falsifies

**Files:**
- Modify: `ts/apps/iam-console/lib/form.ts` — 3 doc comments
- Modify: `ts/apps/iam-console/tests/unit/form.test.ts:44` — 1 comment

**Interfaces:**
- Consumes: the behaviour Tasks 1-3 established.
- Produces: nothing. **No code changes in this task. Comments only.**

Three comments in `form.ts` state the invariant Tasks 1-3 removed. The code they describe is still
correct, but for a different reason: the legacy rows the spec's D4 keeps, not an absent check in
IAM. A reader who verifies IAM, finds `validate_name`, and applies `nameField` unconditionally
would break every slug-only rename of a legacy node.

- [ ] **Step 1: Correct `nameField`'s comment**

At `ts/apps/iam-console/lib/form.ts:20-23`, replace the last sentence. The current text reads:

```
 * IAM's rename path does not validate a name (spec F11), so for a rename this schema is the
 * only guard.
```

Replace with:

```
 * IAM's rename path validates a name too since SMA-642, with the same rule and the same 256
 * code-point bound, so this schema is no longer the only guard for a rename — it is the one
 * that gives the user the error in the form rather than a round trip.
```

- [ ] **Step 2: Correct `currentField`'s comment**

At `ts/apps/iam-console/lib/form.ts:33-39`, the current text reads:

```
 * bound, because IAM stores a renamed name without a check (spec F11). A stricter schema here
 * would refuse every rename of such a node.
```

Replace with:

```
 * bound, because IAM stored renamed names without a check before SMA-642 and those rows are
 * kept as they are (SMA-642 D4). A stricter schema here would refuse every rename of such a
 * node.
```

- [ ] **Step 3: Correct `renameForm`'s comment**

At `ts/apps/iam-console/lib/form.ts:47-53`, the current text reads:

```
 * `name` and the two hidden "current value" fields. IAM can store a name longer than 256 code
 * points (spec F11), so `nameField`'s bound applies ONLY when the trimmed name changed.
```

Replace with:

```
 * `name` and the two hidden "current value" fields. IAM holds names longer than 256 code points
 * that it stored before SMA-642 and does not migrate (SMA-642 D4), so `nameField`'s bound
 * applies ONLY when the trimmed name changed. Do NOT make it unconditional now that IAM
 * validates: that would refuse every slug-only rename of such a node.
```

- [ ] **Step 4: Correct the test comment**

At `ts/apps/iam-console/tests/unit/form.test.ts:86-88`, the current text reads:

```
// SMA-630 CR round 1, spec § 4.2. The `name` bound applies only when the trimmed name changed: IAM
// can store a name longer than 256 code points (spec F11), and a slug-only rename of such a node
// must still work.
```

Replace with:

```
// SMA-630 CR round 1, spec § 4.2, updated by SMA-642. The `name` bound applies only when the
// trimmed name changed: IAM holds names longer than 256 code points that it stored before SMA-642
// validated the rename path, it does not migrate them (SMA-642 D4), and a slug-only rename of such
// a node must still work.
```

Leave the comment at `:42-43` alone. It says the bound counts code points as `NAME_MAX_CHARS`
does, which this change does not affect.

- [ ] **Step 5: Verify no code changed**

```bash
git diff --stat ts/
git diff ts/ | grep -E '^[+-]' | grep -vE '^[+-]{3}' | grep -vE '^[+-]\s*\*|^[+-]\s*//|^[+-]\s*/\*'
```

Expected: the first shows only the two files. The second prints **nothing** — every changed line is
a comment line. If it prints a line, you changed code; revert that line.

- [ ] **Step 6: Run the console's checks**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec prettier --check 'apps/iam-console/lib/form.ts' 'apps/iam-console/tests/unit/form.test.ts'
moon run iam-console-ts:test
```

Expected: both clean. The unit tests must pass unchanged — this task edits no behaviour.

- [ ] **Step 7: Commit**

```bash
git add ts/apps/iam-console/lib/form.ts ts/apps/iam-console/tests/unit/form.test.ts
git commit -m "docs(ts): correct the console comments that SMA-642 falsifies

Three comments in form.ts said IAM does not validate a renamed name.
IAM does now. The conditional bound in renameForm is still needed, but
for the rows IAM stored before the fix and does not migrate.

Stated so a reader who checks IAM and finds validate_name does not make
the bound unconditional, which would refuse every slug-only rename of
such a node.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Final verification

After all five tasks, run the full graph the way CI does. Per `CLAUDE.md`, per-project Moon tasks do
NOT run the repo-level gates, and this change adds a new test to a `tests/` file and touches `ts/`.

- [ ] **Step 1: The full gate command**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :test-e2e \
  --base origin/main --include-relations
```

- [ ] **Step 2: Read the result with the local-bash caveats in mind**

Per `CLAUDE.md`, no single local bash satisfies every gate on this machine:

- `repo:affected-smoke` needs system `/bin/bash` 3.2.
- `repo:ruff-ci` and `repo:next-public-free` need bash 4+ (`mapfile`).
- `repo:actionlint` has **no working local bash**; it does not finish under 3.2 and deadlocks under
  Homebrew 5.3.15. There is no local verdict for it — CI is the verdict.

A wall of "expected rc 0" self-test failures across unrelated gates means the wrong bash ran them,
not a real finding. Check `which -a bash` before reading such a result as a defect.

- [ ] **Step 3: Expected gate outcomes for this change**

- `repo:error-code-single-site` must be GREEN. It reds if any of the three application `src/` files
  gained the literal `"invalid-name"`. Task 4's literal is in a `tests/` file, which is fine.
- `repo:affected-smoke` must be GREEN. This change adds no crate, no dependency and no `repo:*`
  gate, so no registry needs an entry.
- `gateway-console-ts:test-e2e` will run because Task 5 touches `iam-console`. It needs Docker.

- [ ] **Step 4: Confirm the spec's § 6.4 regression rule held**

```bash
git diff origin/main -- rs/crates/services/paigasus-iam/src/application/ | grep -E '^-' | grep -vE '^---'
```

Expected: the only removed lines are the two `rename_in` call lines each task replaced, plus any
`use` line rustfmt reflowed. **No removed line may come from an existing test.** If one does, the
change altered behaviour beyond the spec — stop and report it.

---

## Self-review notes

Checked against the spec:

- § 4 D1 -> Tasks 1, 2, 3 (Steps 3-5 of each). D2 -> Task 1 Steps 2 and 5, tests
  `rename_stores_the_trimmed_name`, `a_whitespace_only_difference_is_a_no_op`,
  `a_legacy_untrimmed_name_is_normalized_by_the_next_rename`. D3 -> Task 1's
  `a_bad_name_outranks_an_archived_node`, Task 2's `a_bad_name_outranks_an_ancestor_archived_node`,
  Task 4 assertion 2. D4 -> Task 1's `a_slug_only_rename_survives_a_legacy_overlong_name`. D5 -> no
  task, by design (nothing to change); the Final verification asserts the registry gate stays
  green. D6 -> no task, by design. D7 -> Task 5.
- § 6.1 -> Task 1, 2, 3 each have `rename_refuses_an_empty_blank_or_overlong_name`.
- § 6.2 -> Task 1 carries seven of the eight rows; the ancestor-archived row is Task 2, because an
  organization has no ancestor to archive.
- § 6.3 -> Task 4.
- § 6.4 -> Task 1 Step 7 and the Final verification Step 4.
- § 6.5 -> the commands in each task, and the Final verification.
- § 7 Risk 3 -> Task 1's `create_and_rename_accept_the_same_names`.

One deliberate deviation from the spec's wording: § 6.2 assigns "the stored name is trimmed" to the
organization only, but Tasks 2 and 3 each repeat it. The reason is specific — a wrong copy of the
fix that validates the name and then still forwards the raw `new_name` passes that file's refusal
test and fails only this one. It is the cheapest guard against the one plausible per-file mistake.
