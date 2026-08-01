# SMA-470 — Authz revocation during a Redis outage: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make role-grant revocation take effect within a bounded, documented window even when the Redis-backed generation counter is unavailable, reset, or flapping — and record the decision not to offer fail-closed authz.

**Architecture:** `CompiledPolicies::r#gen` currently does triple duty (freshness comparator, decision-cache key component, install-ordering token) and is sourced from Redis, so a Redis failure breaks all three at once. This plan splits those duties: the decision-cache key moves to a blake3 **content hash** of the compiled policy set (stable across replicas, immune to counter resets), the install-ordering guard moves to a process-local **sequence number**, and `r#gen` is left as the reload comparator only. `PolicySnapshot` then tolerates an unreadable counter by compiling from Postgres anyway, and single-flight + provisional-stamp guards keep the new "reload on generation inequality" rule from turning into a recompile-per-decision storm.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), Cedar (`cedar-policy` 4.x), SeaORM, `redis` `ConnectionManager`, `blake3`, `tokio`, `metrics`, `tracing`, testcontainers (Postgres + Redis), `cargo nextest`, Moon.

**Spec:** `docs/superpowers/specs/2026-08-01-sma-470-authz-revocation-during-redis-outage-design.md`

## Global Constraints

- Every source file opens with an SPDX header — `// SPDX-License-Identifier: Apache-2.0` for
  Rust/TS/JS/proto, `# SPDX-License-Identifier: Apache-2.0` for Python and YAML.
- Rust crates use **edition 2024 + rust-version 1.95**.
- Conventional commits with a workspace scope: `feat(rs): …`, `fix(rs): …`, `test(rs): …`, `docs(rs): …`.
- Commit subject must **start lowercase** and be **≤100 chars**. Never put a bare `#NNN` in the commit body — write "owner/repo PR NNN". Keep one contiguous footer block.
- Do **not** bypass git hooks with `--no-verify`.
- Bash PATH lacks the proto CLIs — prefix every tooling command with
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- All work happens in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-470-authz-revocation-outage` on branch `feature/sma-470-iam-cover-revocation-during-redis-outage`. Paths below are relative to that root.
- `cargo nextest` needs `--no-tests=pass` on a crate with no tests.
- Fail-open is the contract: no change may make an authorization decision *fail* because Redis is unavailable.

---

### Task 1: Content-hash the compiled policy set

Adds `CompiledPolicies::content_hash`, computed inside the pure engine from the exact documents and grants that were compiled. This is the foundation for Task 4 (which stops keying the decision cache on the generation counter).

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/Cargo.toml` (add `blake3`)
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/engine.rs:36-46` (struct), and `PolicyEngine::compile`'s return
- Test: `rs/crates/libs/paigasus-iam-core/src/authz/engine.rs` (in-file `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  - `CompiledPolicies { pub policy_set: PolicySet, pub r#gen: u64, pub content_hash: String }`
  - `PolicyEngine::compile(&[PolicyDocument], &[RoleGrant]) -> Result<CompiledPolicies, AuthzError>` — unchanged signature, now sets `content_hash` to a 64-char lowercase blake3 hex digest.

- [ ] **Step 1: Add the `blake3` dependency to `paigasus-iam-core`**

In `rs/crates/libs/paigasus-iam-core/Cargo.toml`, under `[dependencies]`, add:

```toml
# `authz::engine::PolicyEngine::compile` — blake3-hashes the canonical encoding of the
# compiled documents + grants into `CompiledPolicies::content_hash`, which keys the
# decision cache (SMA-470). Already a workspace dep via `paigasus-iam`.
blake3 = { workspace = true }
```

- [ ] **Step 2: Write the failing tests**

Append to the `#[cfg(test)] mod tests` block in `rs/crates/libs/paigasus-iam-core/src/authz/engine.rs`. If that module already has helpers named `policy_doc` / `role_grant`, reuse them and drop the local copies below.

```rust
    /// SMA-470 D4: the content hash must be a pure function of the compiled inputs, so two
    /// replicas compiling the same policy set produce the same decision-cache key space.
    #[test]
    fn content_hash_is_stable_for_identical_inputs() {
        let docs = vec![hash_fixture_template()];
        let grants = vec![hash_fixture_grant(Uuid::from_u128(1))];

        let a = PolicyEngine::compile(&docs, &grants).expect("compiles");
        let b = PolicyEngine::compile(&docs, &grants).expect("compiles");

        assert_eq!(a.content_hash, b.content_hash);
        assert_eq!(a.content_hash.len(), 64, "blake3 hex digest is 64 chars");
        assert!(a.content_hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// SMA-470 D4: input ORDER must not change the hash — `list_all` gives no ordering
    /// guarantee, and two replicas reading the same rows in different orders must still
    /// share a cache key space.
    #[test]
    fn content_hash_ignores_input_ordering() {
        let g1 = hash_fixture_grant(Uuid::from_u128(1));
        let g2 = hash_fixture_grant(Uuid::from_u128(2));

        let forward = PolicyEngine::compile(&[hash_fixture_template()], &[g1.clone(), g2.clone()]).expect("compiles");
        let reverse = PolicyEngine::compile(&[hash_fixture_template()], &[g2, g1]).expect("compiles");

        assert_eq!(forward.content_hash, reverse.content_hash);
    }

    /// SMA-470 D4: revoking a grant MUST change the hash — this is what moves the decision
    /// cache to a fresh key space and makes a lost `policy_gen` bump irrelevant to the cache.
    #[test]
    fn content_hash_changes_when_a_grant_is_revoked() {
        let docs = vec![hash_fixture_template()];
        let with_grant = PolicyEngine::compile(&docs, &[hash_fixture_grant(Uuid::from_u128(1))]).expect("compiles");
        let without = PolicyEngine::compile(&docs, &[]).expect("compiles");

        assert_ne!(with_grant.content_hash, without.content_hash);
    }

    /// SMA-470 D4: editing a policy's Cedar source must change the hash even though the
    /// policy id is unchanged.
    #[test]
    fn content_hash_changes_when_a_policy_source_changes() {
        let original = PolicyEngine::compile(&[hash_fixture_template()], &[]).expect("compiles");

        let mut edited_doc = hash_fixture_template();
        edited_doc.source = r#"permit(principal == ?principal, action, resource in ?resource) when { true };"#.to_string();
        let edited = PolicyEngine::compile(&[edited_doc], &[]).expect("compiles");

        assert_ne!(original.content_hash, edited.content_hash);
    }

    /// SMA-470: two DIFFERENT documents must never hash alike just because a field value
    /// contains the row delimiter. `policy_id` and `role_key` are arbitrary caller-chosen
    /// strings with no charset validation, so an unescaped join would let an attacker craft a
    /// policy edit that does NOT rotate the decision-cache key — silently serving stale
    /// authorization decisions. Every field is length-prefixed independently, so the encoding
    /// is unambiguous about where each field ends.
    #[test]
    fn content_hash_is_unambiguous_across_field_boundaries() {
        let mut shifted_into_source = hash_fixture_template();
        shifted_into_source.policy_id = "a".to_string();
        shifted_into_source.source = "b\u{1f}static\u{1f}c".to_string();

        let mut shifted_into_id = hash_fixture_template();
        shifted_into_id.policy_id = "a\u{1f}static\u{1f}b".to_string();
        shifted_into_id.source = "c".to_string();

        // If `compile` rejects these sources at Cedar-parse time both sides are `Err` and the
        // assertion is vacuous — call the private `content_hash` directly in that case.
        assert_ne!(
            content_hash(&[shifted_into_source], &[]),
            content_hash(&[shifted_into_id], &[]),
            "a delimiter inside a field value must not forge another document's digest"
        );
    }

    fn hash_fixture_template() -> PolicyDocument {
        let now = chrono::Utc::now();
        PolicyDocument {
            policy_id: "org_admin".to_string(),
            kind: PolicyKind::Template,
            source: r#"permit(principal == ?principal, action, resource in ?resource);"#.to_string(),
            description: "test fixture".to_string(),
            system: true,
            created_at: now,
            updated_at: now,
        }
    }

    fn hash_fixture_grant(id: Uuid) -> RoleGrant {
        RoleGrant {
            id,
            principal: PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(9)).expect("static test prn parts are valid")),
            role_key: "org_admin".to_string(),
            scope: GrantScope::Root,
            linked_policy_id: format!("grant:{id}"),
            created_at: chrono::Utc::now(),
        }
    }
```

Note: `created_at` is deliberately **not** hashed (see Step 4) — `hash_fixture_grant` calls `Utc::now()` per invocation, so `content_hash_is_stable_for_identical_inputs` would fail if it were.

- [ ] **Step 3: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam-core --lib content_hash
```

Expected: FAIL to compile — `no field content_hash on type CompiledPolicies`.

- [ ] **Step 4: Implement `content_hash`**

In `rs/crates/libs/paigasus-iam-core/src/authz/engine.rs`, extend the struct (keep the existing doc comment, append the new field's):

```rust
#[derive(Debug)]
pub struct CompiledPolicies {
    pub policy_set: PolicySet,
    /// `r#gen` — `gen` is a reserved keyword as of the 2024 edition.
    pub r#gen: u64,
    /// A blake3 hex digest over a canonical encoding of the documents + grants this was
    /// compiled from (SMA-470 D4). Unlike [`Self::r#gen`] — which is a Redis-sourced counter
    /// that can stall, reset to 0, or miss a swallowed bump — this is a pure function of the
    /// compiled content, so it is identical across replicas that compiled the same policy set
    /// and always changes when the policy set does. It is the decision cache key's policy
    /// component; `r#gen` is only the reload-freshness comparator.
    pub content_hash: String,
}
```

Add the hashing helper below `PolicyEngine::compile`:

```rust
/// Canonical, order-independent blake3 digest of the inputs a [`CompiledPolicies`] was built
/// from (SMA-470 D4). Both slices are hashed as SORTED rows of INDIVIDUALLY length-prefixed
/// fields, so the digest is independent of `list_all`'s row order and unambiguous about where
/// each field ends — a value containing any byte sequence, delimiter-like or not, cannot forge
/// another input's digest.
///
/// `created_at` is deliberately excluded from both encodings: it never affects the compiled
/// policy set, so including it would mint a fresh decision-cache key space for a semantically
/// identical policy set (and make the digest non-reproducible across replicas that re-read
/// rows with differing timestamp precision).
fn content_hash(policies: &[PolicyDocument], grants: &[RoleGrant]) -> String {
    fn field(hasher: &mut blake3::Hasher, value: &str) {
        // Length-prefix every field so ("ab", "c") and ("a", "bc") cannot collide. This must be
        // applied per FIELD, never to a pre-joined row — see the row construction below.
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }

    // Rows are arrays of raw fields, NEVER pre-joined strings: every field is length-prefixed
    // INDIVIDUALLY when hashed below. Pre-joining with a delimiter and length-prefixing only the
    // joined row would leave the field boundaries ambiguous — `(policy_id = "a", source =
    // "b<DEL>static<DEL>c")` and `(policy_id = "a<DEL>static<DEL>b", source = "c")` would encode
    // identically, letting a crafted policy edit fail to rotate the decision-cache key.
    // `policy_id`/`role_key` are arbitrary caller-chosen strings with no charset validation, so
    // that is reachable input, not a hypothetical.
    //
    // Sorting arrays of `String` sorts lexicographically field-by-field, which is the canonical
    // order we want and needs no joined key.
    let mut doc_rows: Vec<[String; 3]> = policies
        .iter()
        .map(|d| {
            let kind = match d.kind {
                PolicyKind::Static => "static",
                PolicyKind::Template => "template",
            };
            [d.policy_id.clone(), kind.to_string(), d.source.clone()]
        })
        .collect();
    doc_rows.sort_unstable();

    let mut grant_rows: Vec<[String; 5]> = grants
        .iter()
        .map(|g| {
            [
                g.id.to_string(),
                // The FULL principal PRN, not the bare uuid: `link_grant` binds `?principal` to
                // `to_cedar_uid(grant.principal.prn())`, so hashing the uuid alone would let two
                // grants that compile to DIFFERENT Cedar policies share a decision-cache key.
                g.principal.canonical(),
                g.role_key.clone(),
                g.scope.canonical_prn(),
                g.linked_policy_id.clone(),
            ]
        })
        .collect();
    grant_rows.sort_unstable();

    let mut hasher = blake3::Hasher::new();
    field(&mut hasher, "policies");
    hasher.update(&(doc_rows.len() as u64).to_le_bytes());
    for row in &doc_rows {
        for value in row {
            field(&mut hasher, value);
        }
    }
    field(&mut hasher, "grants");
    hasher.update(&(grant_rows.len() as u64).to_le_bytes());
    for row in &grant_rows {
        for value in row {
            field(&mut hasher, value);
        }
    }
    hasher.finalize().to_hex().to_string()
}
```

Then set it on the value `compile` returns. Find the `Ok(CompiledPolicies { … })` (or equivalent struct literal) at the end of `PolicyEngine::compile` and add the field:

```rust
        Ok(CompiledPolicies {
            policy_set,
            r#gen: 0,
            content_hash: content_hash(policies, grants),
        })
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam-core --lib content_hash
```

Expected: PASS, 4 tests.

- [ ] **Step 6: Build the whole workspace to find every other construction site**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build --workspace --all-targets 2>&1 | tail -40
```

Adding a field breaks any other struct-literal construction of `CompiledPolicies`. Fix each reported site by adding `content_hash: <expr>`; in test fakes, `String::new()` is fine, in production paths call `content_hash(...)`. Re-run until clean.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/libs/paigasus-iam-core/
git commit -m "feat(rs): content-hash the compiled policy set for SMA-470"
```

---

### Task 2: Split the install-ordering token out of the generation stamp

Replaces `install_if_newer`'s `r#gen`-comparison with a process-local sequence number claimed immediately before the first Postgres read. This is what makes the TTL backstop able to install a same-generation recompile (defect D-B).

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/policy_snapshot.rs`
- Test: same file, `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `CompiledPolicies.content_hash` from Task 1 (not used yet; Task 4 wires it).
- Produces:
  - `PolicySnapshot { policies, grants, state: RwLock<SnapshotState>, load_seq: AtomicU64 }`
  - `SnapshotState { compiled: Arc<CompiledPolicies>, loaded_at: Instant, installed_seq: u64 }`
  - `async fn load_and_compile(policies: &dyn PolicyStore, grants: &dyn RoleGrantStore, load_seq: &AtomicU64) -> Result<(CompiledPolicies, u64), AuthzError>` — stays an **associated fn** (so `new()` can call it before `Self` exists) but takes the sequence source, returning `(compiled, seq)`.
  - `async fn install_if_fresher(&self, compiled: CompiledPolicies, seq: u64)` — replaces `install_if_newer`.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `policy_snapshot.rs`:

```rust
    /// SMA-470 D-B: the TTL backstop (`spawn_reload`'s `ttl_elapsed` branch calls
    /// `reload_now` unconditionally) must actually INSTALL a recompile when the generation
    /// counter has not moved — the exact case the module docs say the backstop exists for.
    /// Before this fix `install_if_newer` required a strictly greater gen, so the backstop
    /// recompiled and discarded on every poll, forever, and a grant/revoke whose bump was
    /// swallowed was never picked up.
    #[tokio::test]
    async fn ttl_backstop_installs_a_same_gen_recompile() {
        let policies_store = Arc::new(FakePolicyStore::new(vec![org_admin_template()]));
        let grants_store = Arc::new(FakeRoleGrantStore::new(vec![]));
        let policies: Arc<dyn PolicyStore> = policies_store.clone();
        let grants: Arc<dyn RoleGrantStore> = grants_store.clone();

        let snapshot = PolicySnapshot::new(policies, grants).await.expect("build succeeds");
        assert_eq!(snapshot.current().await.r#gen, 0);

        // A grant lands in Postgres but its `policy_gen` bump is LOST (Redis down) — SMA-470's
        // scenario, in the grant direction so the effect is easy to observe.
        let grant_id = Uuid::from_u128(700);
        grants_store.grant(&role_grant(grant_id, "org_admin")).await.unwrap();

        snapshot.reload_now().await.expect("reload_now succeeds");

        let after = snapshot.current().await;
        assert!(
            after.policy_set.policy(&grant_policy_id(grant_id)).is_some(),
            "the TTL backstop must install a same-gen recompile — otherwise a swallowed bump is never recovered"
        );
    }

    /// SMA-470: the backstop must also reset the TTL clock when it installs, so
    /// `spawn_reload` stops re-entering the `ttl_elapsed` branch on every poll.
    #[tokio::test]
    async fn a_same_gen_backstop_install_resets_the_ttl_clock() {
        let policies: Arc<dyn PolicyStore> = Arc::new(FakePolicyStore::new(vec![org_admin_template()]));
        let grants: Arc<dyn RoleGrantStore> = Arc::new(FakeRoleGrantStore::new(vec![]));

        let snapshot = PolicySnapshot::new(policies, grants).await.expect("build succeeds");
        let before = snapshot.state.read().await.loaded_at;

        tokio::time::sleep(Duration::from_millis(5)).await;
        snapshot.reload_now().await.expect("reload_now succeeds");

        assert!(snapshot.state.read().await.loaded_at > before, "an installed recompile must refresh loaded_at");
    }

    /// SMA-470: the monotonic-write guard's intent is preserved, now expressed over the load
    /// sequence rather than the generation — a load that STARTED earlier must never overwrite
    /// one that started later, even if it reaches the write lock last. This replaces the old
    /// `install_if_newer_rejects_an_older_gen_arriving_after_a_newer_one_is_installed`.
    #[tokio::test]
    async fn install_if_fresher_rejects_an_older_load_arriving_after_a_newer_one() {
        let policies_store = Arc::new(FakePolicyStore::new(vec![org_admin_template()]));
        let grants_store = Arc::new(FakeRoleGrantStore::new(vec![]));
        let policies: Arc<dyn PolicyStore> = policies_store.clone();
        let grants: Arc<dyn RoleGrantStore> = grants_store.clone();

        let snapshot = PolicySnapshot::new(policies, grants).await.expect("build succeeds");

        // The "older" load starts first and so claims the lower seq. `load_and_compile` is an
        // ASSOCIATED fn (it predates `Self`, so `new()` can call it) — pass the stores and the
        // seq source explicitly; there is no `self.load_and_compile(..)` receiver form.
        let (older, older_seq) = PolicySnapshot::load_and_compile(policies_store.as_ref(), grants_store.as_ref(), &snapshot.load_seq)
            .await
            .expect("compiles");

        // A grant lands, then the "newer" load starts and claims the higher seq.
        let grant_id = Uuid::from_u128(900);
        grants_store.grant(&role_grant(grant_id, "org_admin")).await.unwrap();
        policies_store.bump_policy_gen().await.unwrap();
        let (newer, newer_seq) = PolicySnapshot::load_and_compile(policies_store.as_ref(), grants_store.as_ref(), &snapshot.load_seq)
            .await
            .expect("compiles");
        assert!(newer_seq > older_seq);

        // The newer load wins the race to the write lock.
        snapshot.install_if_fresher(newer, newer_seq).await;
        let installed = snapshot.current().await;
        assert!(installed.policy_set.policy(&grant_policy_id(grant_id)).is_some());

        // The older load arrives second — the guard must reject it rather than regress.
        snapshot.install_if_fresher(older, older_seq).await;
        let after = snapshot.current().await;

        assert!(
            after.policy_set.policy(&grant_policy_id(grant_id)).is_some(),
            "an older-seq load must never overwrite a newer installed one"
        );
        assert!(Arc::ptr_eq(&installed, &after), "the rejected install must leave the installed Arc untouched");
    }
```

Delete the existing `install_if_newer_rejects_an_older_gen_arriving_after_a_newer_one_is_installed` test — the third test above replaces it, preserving its intent over the new token.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib policy_snapshot
```

Expected: FAIL to compile — `no method named install_if_fresher`, and `load_and_compile` takes the wrong number of arguments (it has no `load_seq` parameter yet).

- [ ] **Step 3: Implement the sequence-number guard**

In `policy_snapshot.rs`, add the import and extend the two structs:

```rust
use std::sync::atomic::{AtomicU64, Ordering};
```

```rust
struct SnapshotState {
    compiled: Arc<CompiledPolicies>,
    loaded_at: Instant,
    /// The `load_seq` of the load whose result is currently installed (SMA-470). The
    /// monotonic-write guard compares THIS, not `compiled.r#gen`: the generation is a
    /// Redis-sourced counter that can stall, reset, or repeat, so it cannot order two loads
    /// — and requiring it to strictly advance is what made the TTL backstop unable to install
    /// a same-generation recompile (defect D-B).
    installed_seq: u64,
}

pub struct PolicySnapshot {
    policies: Arc<dyn PolicyStore>,
    grants: Arc<dyn RoleGrantStore>,
    state: RwLock<SnapshotState>,
    /// Hands out a strictly increasing token per load. Claimed immediately BEFORE the first
    /// Postgres read (see [`Self::load_and_compile`]) so it orders loads by when they read
    /// their data — which is the property the monotonic-write guard actually needs.
    load_seq: AtomicU64,
}
```

`new()` builds the sequence source first, so the initial load claims seq `1` through the same
path every later reload uses — no throwaway empty compile, no double install:

```rust
    pub async fn new(policies: Arc<dyn PolicyStore>, grants: Arc<dyn RoleGrantStore>) -> Result<Self, AuthzError> {
        let load_seq = AtomicU64::new(0);
        let (compiled, seq) = Self::load_and_compile(policies.as_ref(), grants.as_ref(), &load_seq).await?;
        Ok(Self {
            policies,
            grants,
            state: RwLock::new(SnapshotState {
                compiled: Arc::new(compiled),
                loaded_at: Instant::now(),
                installed_seq: seq,
            }),
            load_seq,
        })
    }
```

Replace `install_if_newer` with:

```rust
    /// Installs `compiled` under the write lock iff `seq` is fresher than the installed load's
    /// — the monotonic-write guard. If it isn't (this load lost a race against one that
    /// started later), `compiled` is dropped and `state` is left completely untouched.
    ///
    /// `loaded_at` moves only on an actual install, so a losing no-op reload can never mask
    /// this replica's true staleness from [`Self::spawn_reload`]'s TTL backstop.
    async fn install_if_fresher(&self, compiled: CompiledPolicies, seq: u64) {
        let mut state = self.state.write().await;
        if seq > state.installed_seq {
            state.compiled = Arc::new(compiled);
            state.loaded_at = Instant::now();
            state.installed_seq = seq;
        } else {
            tracing::debug!(rejected_seq = seq, installed_seq = state.installed_seq, "policy_snapshot: discarding an out-of-order reload");
        }
    }
```

Rewrite `reload_now` and `load_and_compile`. `load_and_compile` **stays an associated fn** — `new()`
calls it before `Self` exists — so it takes the `load_seq` source as an explicit `&AtomicU64`
parameter rather than reaching for `self`:

```rust
    async fn reload_now(&self) -> Result<(), AuthzError> {
        let (compiled, seq) = Self::load_and_compile(self.policies.as_ref(), self.grants.as_ref(), &self.load_seq).await?;
        self.install_if_fresher(compiled, seq).await;
        Ok(())
    }
```

In `load_and_compile`, claim the seq immediately before the first **Postgres** read — i.e. *after*
the `policy_gen()` call, not at the top of the fn — and return it. Keep the rest of the body (the
`skipped_role_keys` warn) exactly as it is:

```rust
    async fn load_and_compile(policies: &dyn PolicyStore, grants: &dyn RoleGrantStore, load_seq: &AtomicU64) -> Result<(CompiledPolicies, u64), AuthzError> {
        let observed_gen = policies.policy_gen().await?;
        // Claim the ordering token here — after the Redis-backed `policy_gen` read, immediately
        // before the first Postgres read — so it orders loads by when they read their data.
        // Claiming it at the top of the fn would put the `policy_gen` read, the step that stalls
        // for whole seconds during exactly the Redis outage this guards against, between the
        // token and the data it labels: a load could hold a LOW seq while it waits and then read
        // NEWER Postgres data than a load holding a higher one, and `install_if_fresher` would
        // discard the fresher policy set as out of order.
        let seq = load_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let docs = policies.list_all().await?;
        let all_grants = grants.list_all().await?;

        let mut compiled = PolicyEngine::compile(&docs, &all_grants)?;
        compiled.r#gen = observed_gen;

        let skipped_role_keys: Vec<&str> = all_grants
            .iter()
            .filter(|g| compiled.policy_set.template(&PolicyId::new(&g.role_key)).is_none())
            .map(|g| g.role_key.as_str())
            .collect();
        if !skipped_role_keys.is_empty() {
            tracing::warn!(
                ?skipped_role_keys,
                total_grants = all_grants.len(),
                "policy_snapshot: grant(s) named a role template absent from the compiled policy set — skipped (fail-safe, contributes no permission)"
            );
        }

        Ok((compiled, seq))
    }
```

Update the `load_and_compile` call sites inside the test module (the mid-load-bump test) to the
new signature and tuple return — they were already
`PolicySnapshot::load_and_compile(policies.as_ref(), grants.as_ref())` and just gain the
`&snapshot.load_seq` argument.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib policy_snapshot
```

Expected: PASS, including the pre-existing `reload_if_stale_*` and mid-load-bump tests.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/authz/policy_snapshot.rs
git commit -m "fix(rs): order policy-snapshot installs by load seq, not generation"
```

---

### Task 3: Tolerate an unreadable, reset, or flapping generation counter

Makes the snapshot reload from Postgres even when Redis is down (D-A), recover from a counter reset (D-C), and refuse to turn either into a recompile-per-decision storm.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/policy_snapshot.rs`
- Test: same file

**Interfaces:**
- Consumes: the associated fn `PolicySnapshot::load_and_compile(policies, grants, load_seq) -> Result<(CompiledPolicies, u64), AuthzError>` and `install_if_fresher` from Task 2.
- Produces:
  - `SnapshotState.stamp_trusted: bool`
  - `PolicySnapshot.reload_gate: tokio::sync::Mutex<()>`
  - `reload_if_stale` semantics: reload on generation **inequality**, single-flight, suppressed while the stamp is provisional.

- [ ] **Step 1: Write the failing tests**

Add to `policy_snapshot.rs`'s test module. First a fake whose `policy_gen()` succeeds once then errors — deliberately *not* "always errors", so "stamped the last-known gen" is distinguishable from "stamped 0":

```rust
    /// A `PolicyStore` fake whose `policy_gen()` returns `Ok(first)` for the first `ok_calls`
    /// calls and errors afterwards — simulating a Redis-backed counter that was readable at
    /// construction and then went away. Erroring from the very first call would make
    /// "stamped the last-known gen" and "stamped 0" indistinguishable.
    struct FlakyGenPolicyStore {
        docs: Mutex<Vec<PolicyDocument>>,
        first: u64,
        ok_calls: AtomicU64,
    }

    impl FlakyGenPolicyStore {
        fn new(docs: Vec<PolicyDocument>, first: u64, ok_calls: u64) -> Self {
            Self { docs: Mutex::new(docs), first, ok_calls: AtomicU64::new(ok_calls) }
        }
    }

    #[async_trait]
    impl PolicyStore for FlakyGenPolicyStore {
        async fn list_all(&self) -> Result<Vec<PolicyDocument>, AuthzError> {
            Ok(self.docs.lock().unwrap().clone())
        }
        async fn put(&self, _doc: &PolicyDocument) -> Result<(), AuthzError> {
            unimplemented!("never written in these tests")
        }
        async fn delete(&self, _policy_id: &str) -> Result<(), AuthzError> {
            unimplemented!("never written in these tests")
        }
        async fn put_in(&self, _tx: &dyn Transaction, _doc: &PolicyDocument) -> Result<PutOutcome, AuthzError> {
            unimplemented!("never written in these tests")
        }
        async fn delete_in(&self, _tx: &dyn Transaction, _policy_id: &str) -> Result<bool, AuthzError> {
            unimplemented!("never written in these tests")
        }
        async fn policy_gen(&self) -> Result<u64, AuthzError> {
            if self.ok_calls.load(Ordering::SeqCst) > 0 {
                self.ok_calls.fetch_sub(1, Ordering::SeqCst);
                Ok(self.first)
            } else {
                Err(AuthzError::Backend("simulated generations-redis outage".into()))
            }
        }
        async fn bump_policy_gen(&self) -> Result<u64, AuthzError> {
            Err(AuthzError::Backend("simulated generations-redis outage".into()))
        }
    }

    /// SMA-470 D-A: a Redis outage must not stop the snapshot reloading. Policies and grants
    /// live in POSTGRES; only the generation stamp comes from Redis, so an unreadable counter
    /// must degrade to a provisional stamp, never to a failed reload.
    #[tokio::test]
    async fn reload_survives_an_unreadable_policy_gen() {
        // One successful read (consumed by `new()`), then the counter goes away.
        let policies_store = Arc::new(FlakyGenPolicyStore::new(vec![org_admin_template()], 5, 1));
        let grants_store = Arc::new(FakeRoleGrantStore::new(vec![]));
        let policies: Arc<dyn PolicyStore> = policies_store.clone();
        let grants: Arc<dyn RoleGrantStore> = grants_store.clone();

        let snapshot = PolicySnapshot::new(policies, grants).await.expect("build succeeds");
        assert_eq!(snapshot.current().await.r#gen, 5);

        // A grant lands while Redis is down — its bump is swallowed.
        let grant_id = Uuid::from_u128(710);
        grants_store.grant(&role_grant(grant_id, "org_admin")).await.unwrap();

        // The TTL backstop still installs fresh Postgres data.
        snapshot.reload_now().await.expect("a Redis outage must not fail the reload");

        let after = snapshot.current().await;
        assert!(after.policy_set.policy(&grant_policy_id(grant_id)).is_some(), "must compile fresh Postgres data with the counter unreadable");
        assert_eq!(after.r#gen, 5, "must stamp the LAST-KNOWN gen, not 0");
    }

    /// SMA-470 D-C: after a Redis data loss the counter reads back 0 (`Generations::read` maps
    /// a missing key to 0). `reload_if_stale` must treat any INEQUALITY as staleness — a
    /// regression means the counter was reset and our stamp is meaningless. Before this fix
    /// `0 <= N` short-circuited and the backstop's `0 > N` install was rejected, freezing the
    /// snapshot until process restart.
    #[tokio::test]
    async fn gen_regression_after_a_redis_flush_still_reloads() {
        let policies_store = Arc::new(FakePolicyStore::new(vec![org_admin_template()]));
        let grants_store = Arc::new(FakeRoleGrantStore::new(vec![]));
        let policies: Arc<dyn PolicyStore> = policies_store.clone();
        let grants: Arc<dyn RoleGrantStore> = grants_store.clone();

        for _ in 0..3 {
            policies_store.bump_policy_gen().await.unwrap();
        }
        let snapshot = PolicySnapshot::new(policies, grants).await.expect("build succeeds");
        assert_eq!(snapshot.current().await.r#gen, 3);

        // Redis is flushed: the counter restarts from 0, then a later grant bumps it to 1.
        policies_store.gen_counter.store(0, Ordering::SeqCst);
        let grant_id = Uuid::from_u128(800);
        grants_store.grant(&role_grant(grant_id, "org_admin")).await.unwrap();
        policies_store.bump_policy_gen().await.unwrap();

        snapshot.reload_if_stale().await.expect("reload succeeds");

        let after = snapshot.current().await;
        assert!(after.policy_set.policy(&grant_policy_id(grant_id)).is_some(), "a reset counter must not freeze the snapshot");
        assert_eq!(after.r#gen, 1, "the snapshot re-stamps to the store's post-reset value");

        // And it settles: equal gens are a cheap no-op, not a reload loop.
        let installed = snapshot.current().await;
        snapshot.reload_if_stale().await.expect("second call succeeds");
        assert!(Arc::ptr_eq(&installed, &snapshot.current().await), "equal gens must not reload");
    }

    /// SMA-470 §3.4 guard 2: while the stamp is PROVISIONAL (the counter was unreadable at the
    /// last load), request-driven reloads are suppressed and refreshing is left to the TTL
    /// backstop. Without this, a flapping Redis — `reload_if_stale`'s read succeeding with N
    /// while `load_and_compile`'s read fails and stamps M != N — yields permanent inequality,
    /// i.e. a full policy recompile on EVERY authorization decision, indefinitely.
    #[tokio::test]
    async fn a_provisional_stamp_suppresses_request_driven_reloads() {
        // `new()` consumes one Ok(5); `reload_now` below then fails its gen read and stamps
        // provisionally; the next `policy_gen()` (from `reload_if_stale`) also errors.
        let policies_store = Arc::new(FlakyGenPolicyStore::new(vec![org_admin_template()], 5, 1));
        let grants_store = Arc::new(FakeRoleGrantStore::new(vec![]));
        let policies: Arc<dyn PolicyStore> = policies_store.clone();
        let grants: Arc<dyn RoleGrantStore> = grants_store.clone();

        let snapshot = PolicySnapshot::new(policies, grants).await.expect("build succeeds");
        snapshot.reload_now().await.expect("backstop reload succeeds with a provisional stamp");
        assert!(!snapshot.state.read().await.stamp_trusted, "the stamp is provisional after an unreadable gen read");

        let installed = snapshot.current().await;
        snapshot.reload_if_stale().await.expect("must not error");
        assert!(
            Arc::ptr_eq(&installed, &snapshot.current().await),
            "a provisional stamp must suppress request-driven reloads — the backstop owns refreshing"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib policy_snapshot
```

Expected: FAIL — `no field stamp_trusted on SnapshotState`, and `reload_survives_an_unreadable_policy_gen` fails because `load_and_compile` still propagates the error.

- [ ] **Step 3: Implement the fallback stamp, inequality reload, and single-flight**

Add to the imports:

```rust
use tokio::sync::Mutex as AsyncMutex;
```

Extend `SnapshotState` and `PolicySnapshot`:

```rust
struct SnapshotState {
    compiled: Arc<CompiledPolicies>,
    loaded_at: Instant,
    installed_seq: u64,
    /// `false` when the installed `compiled.r#gen` is a PROVISIONAL stamp — the counter was
    /// unreadable at load time, so the value was carried over rather than observed. The
    /// compiled policy set itself is still fresh (it came from Postgres); only the stamp is
    /// a guess, so it must not be compared against a live counter read (SMA-470 §3.4).
    stamp_trusted: bool,
}
```

```rust
pub struct PolicySnapshot {
    policies: Arc<dyn PolicyStore>,
    grants: Arc<dyn RoleGrantStore>,
    state: RwLock<SnapshotState>,
    load_seq: AtomicU64,
    /// Single-flight gate: at most one reload runs at a time. `reload_if_stale` `try_lock`s
    /// and gives up immediately if a reload is already in flight, deciding against the current
    /// snapshot instead. Without this, every in-flight request observing the same staleness
    /// runs its own full recompile (a pre-existing herd, SMA-470 §3.4 guard 1).
    reload_gate: AsyncMutex<()>,
}
```

Initialize the two new fields in `new()` (`stamp_trusted: true`, `reload_gate: AsyncMutex::new(())`).

`load_and_compile` gains a `fallback_gen` parameter and returns the trust flag — new signature
`load_and_compile(policies: &dyn PolicyStore, grants: &dyn RoleGrantStore, load_seq: &AtomicU64, fallback_gen: u64) -> Result<(CompiledPolicies, u64, bool), AuthzError>`.
Replace the `policy_gen` read:

```rust
        let (observed_gen, trusted) = match policies.policy_gen().await {
            Ok(g) => (g, true),
            Err(err) => {
                tracing::debug!(error = %err, "policy_snapshot: policy_gen unreadable — compiling from Postgres anyway and stamping the last-known generation (fail-open, SMA-470)");
                (fallback_gen, false)
            }
        };
        // The seq claim from Task 2 stays exactly where it is: BELOW this match, immediately
        // above the first `list_all`. It must not migrate to the top of the fn — that would put
        // the stalling `policy_gen` read between the token and the Postgres data it labels.
        let seq = load_seq.fetch_add(1, Ordering::SeqCst) + 1;
```

and return `Ok((compiled, seq, trusted))`. `new()` passes `0` for `fallback_gen` (nothing is
installed yet) and stores the returned `trusted` in `SnapshotState`.

**Transition logging lives in `install_if_fresher`,** which is the only place that can see both
the old and new flag. This keeps the per-attempt line at `debug` (above) and emits exactly one
`warn`/`info` per state change — at `refresh_interval_secs = 1`, a per-attempt `warn` would be
≥1 line/second/replica for the whole outage.

`install_if_fresher` takes and stores the flag —
`async fn install_if_fresher(&self, compiled: CompiledPolicies, seq: u64, trusted: bool)`. Inside
the install branch, before assigning:

```rust
            match (state.stamp_trusted, trusted) {
                (true, false) => tracing::warn!("policy_snapshot: policy_gen unreadable — serving a Postgres-compiled snapshot on a provisional generation stamp (fail-open, SMA-470)"),
                (false, true) => tracing::info!("policy_snapshot: policy_gen readable again — the generation stamp is authoritative"),
                _ => {}
            }
            state.stamp_trusted = trusted;
```

`reload_now` becomes single-flight-aware but always reloads (the backstop must never be skipped):

```rust
    async fn reload_now(&self) -> Result<(), AuthzError> {
        let _guard = self.reload_gate.lock().await;
        let fallback_gen = self.state.read().await.compiled.r#gen;
        let (compiled, seq, trusted) = Self::load_and_compile(self.policies.as_ref(), self.grants.as_ref(), &self.load_seq, fallback_gen).await?;
        self.install_if_fresher(compiled, seq, trusted).await;
        Ok(())
    }
```

`reload_if_stale` gets both guards and the inequality rule:

```rust
    /// Reloads iff the store's `policy_gen` differs from the currently-compiled `r#gen`;
    /// otherwise a cheap no-op.
    ///
    /// **Inequality, not advance** (SMA-470 D-C): a counter that went BACKWARDS means Redis
    /// was reset (`Generations::read` maps a missing key to 0), so the installed stamp is
    /// meaningless and re-stamping is correct. It settles after one reload, because the new
    /// stamp then equals the store's value.
    ///
    /// Two guards keep that from becoming a recompile-per-decision storm: a provisional stamp
    /// (the counter was unreadable at the last load) suppresses request-driven reloads
    /// entirely, and the single-flight gate caps concurrent reloads at one.
    pub async fn reload_if_stale(&self) -> Result<(), AuthzError> {
        let (current_gen, trusted) = {
            let state = self.state.read().await;
            (state.compiled.r#gen, state.stamp_trusted)
        };
        if !trusted {
            // The installed stamp is a carried-over guess; comparing it to a live read would
            // report permanent staleness. The TTL backstop owns refreshing until the counter
            // is readable again.
            return Ok(());
        }
        let store_gen = self.policies.policy_gen().await?;
        if store_gen == current_gen {
            return Ok(());
        }
        // Give up immediately if a reload is already in flight rather than queueing behind it:
        // this call's decision is served from the current snapshot either way.
        let Ok(_guard) = self.reload_gate.try_lock() else {
            return Ok(());
        };
        // TOCTOU re-check. The `policy_gen()` read above is a Redis round-trip and the gate was
        // NOT held across it; while Redis flaps it stalls for seconds, and a concurrent
        // `reload_now` (the backstop takes this same gate with a blocking `lock`) can install
        // inside that window. Without re-reading, a stamp that has just gone provisional is
        // ignored and every request stalled in `policy_gen` recompiles the whole policy set —
        // the exact per-decision storm the `!trusted` guard exists to prevent.
        let (current_gen, trusted) = {
            let state = self.state.read().await;
            (state.compiled.r#gen, state.stamp_trusted)
        };
        if !trusted || store_gen == current_gen {
            return Ok(());
        }
        let (compiled, seq, stamp_trusted) = Self::load_and_compile(self.policies.as_ref(), self.grants.as_ref(), &self.load_seq, current_gen).await?;
        self.install_if_fresher(compiled, seq, stamp_trusted).await;
        Ok(())
    }
```

Update `new()` and every test call site for the new 4-argument / 3-tuple form.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib policy_snapshot
```

Expected: PASS, all tests including Task 2's.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/authz/policy_snapshot.rs
git commit -m "fix(rs): reload the policy snapshot from postgres when redis is unreadable"
```

---

### Task 4: Key the decision cache on policy content, not the generation counter

Task 3 made the installed `r#gen` non-monotonic. `r#gen` is currently the decision-cache key's policy component, consulted *before* evaluation — so without this task a stamp dropping 7 → 0 re-enters a previously-live key space and can serve a pre-revoke `Allow` ahead of a snapshot that correctly denies. **Task 3 must not ship without Task 4.**

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/decision_cache.rs:27-48` (`decision_key`) and its tests
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/cedar_authorizer.rs:134-149,:170` (`cache_key`)
- Test: both files' `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `CompiledPolicies.content_hash: String` (Task 1); `PolicySnapshot` reload semantics (Task 3).
- Produces: `pub fn decision_key(policy_content: &str, entity_gen: u64, req: &AccessRequest) -> String`.

- [ ] **Step 1: Write the failing tests**

In `decision_cache.rs`'s test module, **delete** `decision_key_changes_when_policy_gen_changes` and update every other `decision_key(1, 2, &req)` call to `decision_key("content-a", 2, &req)`. Then add:

```rust
    /// SMA-470 D4: the key's policy component is the compiled set's content hash, so any
    /// policy/grant change mints a disjoint key space — even when the generation counter did
    /// not move (a swallowed bump, a reset counter).
    #[test]
    fn decision_key_changes_when_policy_content_changes() {
        let req = base_request();
        assert_ne!(decision_key("content-a", 2, &req), decision_key("content-b", 2, &req));
    }

    /// SMA-470 D4: identical content on two replicas yields an identical key, so the Redis
    /// decision cache stays SHARED across the fleet — the property a process-local counter
    /// could never provide.
    #[test]
    fn decision_key_is_stable_across_replicas_for_identical_content() {
        let req = base_request();
        assert_eq!(decision_key("content-a", 2, &req), decision_key("content-a", 2, &req));
    }
```

In `cedar_authorizer.rs`'s test module add:

```rust
    /// SMA-470: the end of the revocation chain with the generation signal MISSING. A grant is
    /// revoked and the `policy_gen` bump is swallowed (Redis down), so nothing about the
    /// counter changes — only the compiled content does. The next decision must be DENY.
    ///
    /// This test is only passable because of D4: with a gen-keyed cache the key would be
    /// byte-identical across the revoke and `MemoryDecisionCache` would replay the cached
    /// `Allow` before evaluation ever ran.
    #[tokio::test]
    async fn revoked_grant_stops_being_allowed_once_the_snapshot_reloads_without_a_gen_bump() {
        let fx = fixture();
        let grant_id = u(600);
        let gens = Generations::memory();
        let policies: Arc<dyn PolicyStore> = Arc::new(FakePolicyStore::new(starter_policies(), gens.clone()));
        let grants_store = Arc::new(FakeRoleGrantStore::new(vec![org_admin_grant(grant_id, &fx.principal, &fx.org)]));
        let grants: Arc<dyn RoleGrantStore> = grants_store.clone();
        let snapshot = Arc::new(PolicySnapshot::new(policies, grants).await.expect("snapshot builds"));
        let slices = Arc::new(FixtureSliceLoader::new(fx.slice.clone()));
        let audit = Arc::new(CapturingAuditSink::default());

        let authorizer = CedarAuthorizer::new(
            snapshot.clone(),
            slices as Arc<dyn EntitySliceLoader>,
            Arc::new(MemoryDecisionCache::new()) as Arc<dyn DecisionCache>,
            Arc::new(gens) as Arc<dyn GenerationsReader>,
            audit.clone() as Arc<dyn AuditSink>,
        );

        let req = base_request(&fx, Action::CreateProject);
        let before = authorizer.is_authorized(&req).await.expect("decision succeeds");
        assert_eq!(before.effect, Effect::Allow, "the grant is in force");

        // Revoke, WITHOUT bumping policy_gen — the Redis-down path.
        grants_store.revoke(grant_id).await.expect("revoke succeeds");
        snapshot.reload_now_for_test().await.expect("the TTL backstop reload succeeds");

        let after = authorizer.is_authorized(&req).await.expect("decision succeeds");
        assert_eq!(after.effect, Effect::Deny, "a revoked grant must not keep being allowed once the snapshot reloaded");
    }
```

`reload_now` is private to `policy_snapshot.rs`, so expose a `#[cfg(test)]`-gated wrapper on `PolicySnapshot` for cross-module tests:

```rust
    /// Test-only access to the TTL backstop's unconditional reload, so sibling modules'
    /// tests (`cedar_authorizer`) can drive it without making `reload_now` public.
    #[cfg(test)]
    pub(crate) async fn reload_now_for_test(&self) -> Result<(), AuthzError> {
        self.reload_now().await
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
# `cargo test`'s filter is a SUBSTRING, not a regex — "a|b" matches nothing at all and exits 0
# with "0 tests run", which reads exactly like a pass. Run the two filters separately.
cd rs && cargo test -p paigasus-iam --lib decision_key
cd rs && cargo test -p paigasus-iam --lib revoked_grant_stops
```

Expected: FAIL to compile — `decision_key` expects `u64` for its first parameter.

- [ ] **Step 3: Implement the content-keyed cache**

In `decision_cache.rs`, change the key function and its doc comment:

```rust
/// The cache key for one [`AccessRequest`] decided against a given compiled-policy content
/// hash and `entity_gen`: `iam:authz:dec:<policy_content>:<entity_gen>:<blake3 hex digest>`.
///
/// The policy component is [`CompiledPolicies::content_hash`], NOT the `policy_gen` counter
/// (SMA-470 D4). The counter is Redis-sourced: it can stall behind a swallowed bump, reset to
/// 0 when Redis loses its data, and therefore move NON-monotonically — which would let a key
/// space that was live earlier be re-entered, replaying a pre-revoke `Allow` from before the
/// change. A content hash cannot: it is a pure function of the compiled policy set, identical
/// across replicas that compiled the same set (so the cache stays shared fleet-wide) and
/// always different when the set differs.
#[must_use]
pub fn decision_key(policy_content: &str, entity_gen: u64, req: &AccessRequest) -> String {
    let canonical = (req.principal.canonical(), req.action.as_wire(), req.resource.canonical(), &req.context);
    let bytes = serde_json::to_vec(&canonical).expect("decision_key's canonical tuple is always serializable");
    let digest = blake3::hash(&bytes);
    format!("{KEY_PREFIX}{policy_content}:{entity_gen}:{}", digest.to_hex())
}
```

In `cedar_authorizer.rs`, change `cache_key`'s first parameter and the call site:

```rust
    async fn cache_key(&self, policy_content: &str, req: &AccessRequest) -> Option<String> {
        match self.gens.entity_gen().await {
            Ok(entity_gen) => Some(decision_key(policy_content, entity_gen, req)),
            Err(err) => {
                tracing::warn!(error = %err, "cedar_authorizer: entity generation counter unreadable — bypassing the decision cache for this call (fail-open, D11/D12)");
                None
            }
        }
    }
```

and at the call site (currently `self.cache_key(compiled.r#gen, req)`):

```rust
        let cache_key = self.cache_key(&compiled.content_hash, req).await;
```

Update `cedar_authorizer.rs`'s module doc step 2/3 and `cache_key`'s doc comment to say the key's policy component is `compiled.content_hash` (the content of the exact snapshot evaluated), replacing the `compiled.r#gen` wording. The existing
`decision_cache_key_uses_the_evaluated_snapshot_gen_not_a_live_policy_gen_read` test still passes and still guards the right property (no live `policy_gen()` read on the decision path) — rename it to `..._uses_the_evaluated_snapshot_content_not_a_live_policy_gen_read` and update its comment.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib authz
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/authz/
git commit -m "fix(rs): key the authz decision cache on policy content, not generation"
```

---

### Task 5: Docker-free test of the real `spawn_reload` backstop

`spawn_reload` has no test anywhere in the crate — which is why D-B survived. A paused-clock test drives the real loop deterministically with no Docker.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/policy_snapshot.rs` (test module only)

**Interfaces:**
- Consumes: everything from Tasks 2-3.
- Produces: nothing consumed later.

- [ ] **Step 1: Write the failing test**

```rust
    /// SMA-470: drives the REAL `spawn_reload` loop on a paused clock — no Docker, no sleeps,
    /// fully deterministic. Pins the TTL backstop end to end: once `ttl` elapses with no
    /// generation movement, the loop must install a fresh recompile (the D-B regression that
    /// `spawn_reload` had no test for at all).
    #[tokio::test(start_paused = true)]
    async fn spawn_reload_backstop_installs_a_recompile_when_the_gen_never_moves() {
        let policies: Arc<dyn PolicyStore> = Arc::new(FakePolicyStore::new(vec![org_admin_template()]));
        let grants_store = Arc::new(FakeRoleGrantStore::new(vec![]));
        let grants: Arc<dyn RoleGrantStore> = grants_store.clone();

        let snapshot = Arc::new(PolicySnapshot::new(policies, grants).await.expect("build succeeds"));

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let handle = snapshot.clone().spawn_reload(Duration::from_secs(30), Duration::from_secs(1), async move {
            let _ = rx.await;
        });

        // A grant lands with its bump swallowed — nothing about the counter changes.
        let grant_id = Uuid::from_u128(950);
        grants_store.grant(&role_grant(grant_id, "org_admin")).await.unwrap();

        // Yield FIRST, so the spawned loop reaches its `sleep(poll)` and registers the timer.
        // Under a paused clock a timer created AFTER an `advance` is scheduled relative to the
        // already-advanced now, so advancing before the loop has registered its sleep fires
        // nothing at all and the test passes/fails for the wrong reason.
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        // Advance past the TTL so the loop takes its unconditional-reload branch.
        tokio::time::advance(Duration::from_secs(31)).await;
        // Yield enough times for the woken task to run its reload to completion.
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }

        assert!(
            snapshot.current().await.policy_set.policy(&grant_policy_id(grant_id)).is_some(),
            "the backstop loop must install a recompile once the TTL elapses, even with no gen movement"
        );

        // Own the task's lifetime — never leave it running past the test.
        let _ = tx.send(());
        handle.await.expect("the reload loop exits cleanly on shutdown");
    }
```

**The paused clock cannot drive the TTL check itself** (found while implementing; the shipped test
does this). `SnapshotState::loaded_at` is a `std::time::Instant`, and `tokio::time`'s pause/advance
is virtual — it mocks `tokio::time::Instant`, never std's. Advancing past `ttl` therefore leaves
`loaded_at.elapsed()` at a few microseconds and the loop takes its `reload_if_stale` branch
instead. Back-date `loaded_at` explicitly before ticking the loop:

```rust
        {
            let mut state = snapshot.state.write().await;
            state.loaded_at = state.loaded_at.checked_sub(ttl).expect("the process monotonic clock is older than the test's ttl");
        }
```

The clock still stays paused, so the poll tick is fired on demand rather than waited out.

- [ ] **Step 2: Run it to verify it fails against the pre-fix behavior**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib spawn_reload_backstop
```

Expected: PASS (Tasks 2-3 already fixed the behavior). To confirm the test has teeth, temporarily revert `install_if_fresher`'s comparison to `compiled.r#gen > state.compiled.r#gen`, re-run and see it FAIL, then restore. Do not commit the temporary revert.

- [ ] **Step 3: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/authz/policy_snapshot.rs
git commit -m "test(rs): pin the policy-snapshot ttl backstop on a paused clock"
```

---

### Task 6: Snapshot-reload telemetry (metric, panel, alert)

Nothing today reveals a D-B regression: the existing `iam_authz_decisions_total{cache="bypass"}` has no panel and no alert, and it measures the `entity_gen` read, not backstop health.

**Files:**
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs` (const + `ALL`)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/policy_snapshot.rs` (emit)
- Modify: `ops/observability/prometheus/rules/iam.rules.yml` (alert)
- Modify: `ops/observability/grafana/dashboards/iam.json` (panel)
- Modify: `docs/ops/RUNBOOK-observability.md` (catalog row §2.2 + alert entry §4)

**Interfaces:**
- Consumes: `install_if_fresher` / `reload_now` from Tasks 2-3.
- Produces: `paigasus_observability::names::IAM_AUTHZ_POLICY_SNAPSHOT_RELOADS_TOTAL` = `"iam_authz_policy_snapshot_reloads_total"`, labels `outcome` ∈ `installed` | `rejected` | `failed`.

- [ ] **Step 1: Register the metric name**

In `rs/crates/libs/paigasus-observability/src/names.rs`, after `IAM_AUTHZ_DECISIONS_TOTAL`:

```rust
pub const IAM_AUTHZ_POLICY_SNAPSHOT_RELOADS_TOTAL: &str = "iam_authz_policy_snapshot_reloads_total";
```

and add `IAM_AUTHZ_POLICY_SNAPSHOT_RELOADS_TOTAL,` to the `ALL` array after `IAM_AUTHZ_DECISIONS_TOTAL,`.

- [ ] **Step 2: Write the failing test**

In `policy_snapshot.rs`'s test module:

```rust
    /// SMA-470 D5: every reload outcome is counted, so a regression of the TTL backstop
    /// (which would show as reloads that never reach `outcome="installed"`) is visible.
    #[tokio::test]
    async fn reload_records_the_snapshot_reload_counter() {
        let handle = paigasus_observability::init("test-policy-snapshot-reload-metric");
        let policies: Arc<dyn PolicyStore> = Arc::new(FakePolicyStore::new(vec![org_admin_template()]));
        let grants: Arc<dyn RoleGrantStore> = Arc::new(FakeRoleGrantStore::new(vec![]));

        let snapshot = PolicySnapshot::new(policies, grants).await.expect("build succeeds");
        snapshot.reload_now().await.expect("reload succeeds");

        let out = handle.render();
        assert!(out.contains("iam_authz_policy_snapshot_reloads_total"), "expected the reload counter:\n{out}");
        assert!(out.contains(r#"outcome="installed""#), "expected an outcome=\"installed\" label:\n{out}");
    }
```

- [ ] **Step 3: Run it to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib reload_records_the_snapshot_reload_counter
```

Expected: FAIL — the metric is absent from the rendered output.

- [ ] **Step 4: Emit the metric**

Add to `policy_snapshot.rs`'s imports:

```rust
use metrics::counter;
use paigasus_observability::names;
```

In `install_if_fresher`, increment in both branches:

```rust
        if seq > state.installed_seq {
            // … existing install …
            counter!(names::IAM_AUTHZ_POLICY_SNAPSHOT_RELOADS_TOTAL, "outcome" => "installed").increment(1);
        } else {
            tracing::debug!(rejected_seq = seq, installed_seq = state.installed_seq, "policy_snapshot: discarding an out-of-order reload");
            counter!(names::IAM_AUTHZ_POLICY_SNAPSHOT_RELOADS_TOTAL, "outcome" => "rejected").increment(1);
        }
```

In `reload_now`, count a failed load (bounded cardinality — the error text is never a label):

```rust
        let (compiled, seq, trusted) = match Self::load_and_compile(self.policies.as_ref(), self.grants.as_ref(), &self.load_seq, fallback_gen).await {
            Ok(v) => v,
            Err(err) => {
                counter!(names::IAM_AUTHZ_POLICY_SNAPSHOT_RELOADS_TOTAL, "outcome" => "failed").increment(1);
                return Err(err);
            }
        };
```

Apply the same `failed` counting to `reload_if_stale`'s `load_and_compile` call.

- [ ] **Step 5: Run the test to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib policy_snapshot
```

Expected: PASS.

- [ ] **Step 6: Add the alert**

In `ops/observability/prometheus/rules/iam.rules.yml`, after the `IamOutboxRelayStalled` rule:

Every piece of this expression is load-bearing (design §7a amendment D); do not "simplify" any of
it back — a promtool fixture fails for each.

- **`or (up{job="iam"} == 1) * 0`** — `increase()`/`sum()` over a series that has NEVER been
  emitted return an EMPTY vector, and `empty == 0` is also empty, so a naked
  `sum(increase(...)) == 0` goes SILENT exactly when the backstop is dead and
  `outcome="installed"` never fired at all. The `or` supplies the missing zero. It is derived from
  `up` rather than a bare `vector(0)` for two reasons: `* 0` drops `__name__` and leaves a 0-valued
  series carrying exactly `{job, instance}`, which matches the left side's label set so `or`
  composes per target; and `== 1` excludes DOWN targets, which are `TargetDown`'s alert — an
  unlabelled `vector(0)` fires for those too and double-pages one fault.
- **`sum by (job, instance)`** — a bare `sum()` drops the target labels, so one healthy replica's
  installs keep the fleet total non-zero while another replica sits wedged, serving revoked grants.
- **`[10m]` with `for: 5m`** — 15 minutes of total detection, which is what the annotation
  promises. `increase(...[15m])` cannot reach zero until 15m after the last install, so pairing it
  with `for: 15m` pages ~30 minutes late.

```yaml
      - alert: IamPolicySnapshotReloadsStalled
        expr: (sum by (job, instance) (increase(iam_authz_policy_snapshot_reloads_total{outcome="installed"}[10m])) or (up{job="iam"} == 1) * 0) == 0
        for: 5m
        labels: { severity: critical }
        annotations: { summary: "IAM policy snapshot has not installed a reload (revocations may not take effect)", description: "No policy-snapshot reload has been INSTALLED in 15 minutes on {{ $labels.job }}/{{ $labels.instance }}. The snapshot's TTL backstop (authz.policy_cache_ttl_secs, default 30s) should install one every TTL regardless of generation movement, so silence here means role revocations are not taking effect on this replica. Check for outcome=\"failed\" (Postgres unreachable, or a malformed policy row aborting every compile) and outcome=\"rejected\". See RUNBOOK section 4." }
```

The promtool fixtures in `ops/observability/prometheus/rules/tests/iam.test.yml` must cover the
absent-series case, a **flat** series (installs happened, then stopped), a **masked replica** (two
instances, one installing and one flat — exactly one alert, naming the flat instance), and a
**down** target (must NOT fire this alert). Every fixture needs an `up{job="iam", instance=...}`
series, because that is what the `or` branch keys off.

- [ ] **Step 7: Add the dashboard panel**

In `ops/observability/grafana/dashboards/iam.json`, append to the `panels` array (next free `id` is `15`; the existing grid ends at `y: 40`):

```json
    {
      "id": 15,
      "type": "timeseries",
      "title": "Policy snapshot reloads",
      "description": "iam_authz_policy_snapshot_reloads_total by outcome — 'installed' must stay non-zero (SMA-470)",
      "gridPos": { "h": 8, "w": 24, "x": 0, "y": 48 },
      "datasource": { "type": "prometheus", "uid": "prometheus" },
      "fieldConfig": { "defaults": { "unit": "ops" }, "overrides": [] },
      "targets": [
        {
          "refId": "A",
          "datasource": { "type": "prometheus", "uid": "prometheus" },
          "expr": "sum(rate(iam_authz_policy_snapshot_reloads_total[$__rate_interval])) by (outcome)",
          "legendFormat": "{{outcome}}"
        }
      ]
    }
```

- [ ] **Step 8: Run the drift + promtool gates**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-observability --test drift
cd .. && moon run repo:promtool
```

Expected: both PASS. The drift test extracts `iam_`-prefixed tokens from the committed dashboard/rule `expr`s and asserts each is in `ALL` — Step 1 is what makes this pass.

- [ ] **Step 9: Add the RUNBOOK catalog row and alert entry**

In `docs/ops/RUNBOOK-observability.md` §2.2, after the `iam_authz_decisions_total` row:

```markdown
| `iam_authz_policy_snapshot_reloads_total` | counter | `outcome` | Every `PolicySnapshot` reload attempt. `outcome` ∈ `installed` (a fresher compiled set replaced the live one), `rejected` (an out-of-order reload lost its race and was discarded — benign in isolation), `failed` (the load or Cedar compile errored; the last-known-good snapshot keeps serving). `installed` must stay non-zero: the TTL backstop installs one every `authz.policy_cache_ttl_secs` regardless of generation movement, and silence means revocations are not taking effect (SMA-470). |
```

In §4, after the `IamOutboxRelayStalled` entry, add an `### IamPolicySnapshotReloadsStalled` section following the neighbouring entries' shape: what fired, why it matters (revocations silently not applying), first checks (`outcome="failed"` rate, Postgres reachability, `policy_snapshot` warns for a malformed policy row), and the link to §4 "Authz availability posture".

- [ ] **Step 10: Commit**

```bash
git add rs/crates/libs/paigasus-observability/ rs/crates/services/paigasus-iam/ ops/observability/ docs/ops/RUNBOOK-observability.md
git commit -m "feat(rs): add policy-snapshot reload telemetry with panel and alert"
```

---

### Task 7: Acceptance test — revoke during a real Redis outage

The test SMA-470 says is missing, against real Postgres + real Redis.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/authz_acceptance.rs`

**Interfaces:**
- Consumes: everything from Tasks 1-4. `AppState::snapshot() -> Arc<PolicySnapshot>` is already public (`adapters/http/mod.rs:243`); `spawn_reload(ttl, poll, shutdown) -> JoinHandle<()>` is public.
- Produces: nothing consumed later.

- [ ] **Step 1: Write the failing test**

Append to `rs/crates/services/paigasus-iam/tests/authz_acceptance.rs`, following the existing Redis fail-open test's shape (`:540-583`) for container/IdP/config setup. Read that test first and mirror its helpers (`start_redis`, `support::start_mock_idp`, `support::test_config`, `app_with_config`, `send`, `provision`) exactly rather than inventing new ones.

```rust
/// SMA-470: the revocation-during-outage case the issue was filed for. A role is granted and
/// takes effect; Redis then goes away, so the `policy_gen` bump the revoke issues is SWALLOWED
/// (`GenerationsPolicyGenBumper::bump` logs and returns). The revoke itself still commits to
/// Postgres — so the ONLY thing that can recover the decision is the policy snapshot's
/// unconditional TTL backstop, reloading from Postgres with the counter unreadable.
///
/// Driven at the CONFIGURED ttl/poll (1s/1s), not an arbitrarily fast interval, so this
/// measures the documented bound rather than mere liveness.
#[tokio::test]
async fn revoke_during_a_redis_outage_denies_once_the_snapshot_backstop_reloads() {
    let Some((db, _pg_node)) = support::start_migrated_postgres().await else {
        return;
    };
    let Some((redis_node, redis_url)) = start_redis().await else {
        return;
    };
    let idp = support::start_mock_idp().await;

    let mut cfg = support::test_config(&idp);
    cfg.authz.cache = AuthzCacheConfig {
        backend: AuthzCacheBackend::Redis,
        redis_url: Some(redis_url),
    };
    // The harness never calls `IamConfig::validate`, so these bounds are this test's own
    // responsibility: both must be >= 1 and refresh <= ttl for the production config to be legal.
    cfg.authz.policy_cache_ttl_secs = 1;
    cfg.authz.refresh_interval_secs = 1;

    let (app, state) = app_with_config(db, &cfg).await;

    let admin_token = idp.bearer("sma470-admin", Some("sma470-admin@example.com"), "paigasus", 3600);
    let admin_prn = provision(&state, &admin_token).await;

    // Grant a role that makes the decision ALLOW, and prove it took effect while Redis is up.
    let (status, granted) = send(
        &app,
        "POST",
        "/v1/authz/role-grants",
        Some(json!({"principal_prn": admin_prn, "role_key": "org_admin", "scope": {"kind": "root"}})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{granted}");
    let grant_id = granted["id"].as_str().expect("grant id").to_string();

    let (status, allowed) = send(
        &app,
        "POST",
        "/v1/authz/is-authorized",
        Some(json!({"principal_prn": admin_prn, "action": "ListOrganizations", "resource_prn": root_prn().canonical()})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{allowed}");
    assert_eq!(allowed["allowed"], true, "the grant must take effect before the outage: {allowed}");

    // Redis goes away. From here every `policy_gen`/`entity_gen` read errors, so the decision
    // cache is bypassed and the revoke's bump is swallowed.
    redis_node.stop_with_timeout(Some(0)).await.expect("stop redis container");

    let (status, revoked) = send(&app, "DELETE", &format!("/v1/authz/role-grants/{grant_id}"), None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "a revoke must still succeed with Redis down (fail-open): {revoked}");

    // Only the TTL backstop can recover this. Run it at the configured cadence.
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let reload_task = state.snapshot().spawn_reload(Duration::from_secs(1), Duration::from_secs(1), async move {
        let _ = shutdown_rx.await;
    });

    // ttl + poll + generous slack for the compile, then assert. Poll rather than sleeping the
    // whole budget so a healthy run finishes fast and a regression still fails loudly.
    let mut denied = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let (status, decision) = send(
            &app,
            "POST",
            "/v1/authz/is-authorized",
            Some(json!({"principal_prn": admin_prn, "action": "ListOrganizations", "resource_prn": root_prn().canonical()})),
            Some(admin_token.as_str()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "a Redis outage must never fail the request: {decision}");
        if decision["allowed"] == false {
            denied = true;
            break;
        }
    }

    // Own the loop's lifetime: it must not outlive the testcontainers this test is dropping.
    let _ = shutdown_tx.send(());
    reload_task.await.expect("the reload loop exits cleanly");

    assert!(
        denied,
        "a grant revoked during a Redis outage must stop being ALLOWed once the snapshot's TTL backstop reloads from Postgres"
    );
}
```

Adjust the grant request body and the `role_key`/`scope` shape to whatever `create_role_grant` actually accepts — read `adapters/http/authz.rs:47,129` and the existing `authz_role_grants.rs` integration test for the exact DTO before writing this, and use `StatusCode::OK` instead of `CREATED`/`NO_CONTENT` if that is what the handlers return. Also mirror `start_migrated_postgres`'s real tuple order (`(node, db)`).

**Do NOT poll over HTTP** (found while implementing; the shipped test does not). With Redis
stopped, every `is-authorized` request pays the client's full reconnect-retry budget on each of the
several counter reads a decision performs — **~20–30 s per request**, so the sketch's
`for _ in 0..40 { sleep(250ms); … }` is not a 10-second loop, it is a quarter-hour of proving
nothing extra. Watch the **in-process** snapshot instead and assert the decision over HTTP exactly
once, at the end:

```rust
    // `content_hash` is a pure function of the compiled documents + grants (Task 4), so "the
    // backstop installed a recompile that saw the revoke" is exactly "this string changed" —
    // with no dependency on `r#gen`, the counter the outage makes unreadable.
    let hash_before_revoke = state.snapshot().current().await.content_hash.clone();
    // ... stop Redis, revoke, spawn_reload ...
    let install_budget = Duration::from_secs(90);
    let started = std::time::Instant::now();
    let mut installed = false;
    while started.elapsed() < install_budget {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if state.snapshot().current().await.content_hash != hash_before_revoke {
            installed = true;
            break;
        }
    }
```

The budget is a failure **deadline**, not an assertion of the documented bound — the loop exits the
moment it observes the recompile, so widening it costs nothing on the happy path. 90 s rather than
`ttl + poll`-ish because a single failed `policy_gen` read eats ~20–30 s of it before the Postgres
loads and Cedar compile even start, and CI runners are slower than a dev laptop.

- [ ] **Step 2: Run it**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test authz_acceptance revoke_during_a_redis_outage --no-capture
```

Expected: PASS (requires Docker; the test returns early with a note if unavailable). If it fails on the DTO shape, fix the request bodies per Step 1's note — not the production code.

- [ ] **Step 3: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/authz_acceptance.rs
git commit -m "test(rs): cover role revocation during a redis outage"
```

---

### Task 8: Rewrite the RUNBOOK's authz-availability posture

The section currently promises a TTL backstop that installed nothing, and says fail-closed was "deferred" when it is now decided against.

**Files:**
- Modify: `docs/ops/RUNBOOK-observability.md:533-554`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/policy_snapshot.rs` module docs (`:13-20`, `:35-43`)

**Interfaces:** none.

- [ ] **Step 1: Replace §4 "Authz availability posture"**

Keep the heading. Replace the body from `**Fail-open on Redis outage, never fail-closed.**` through the end of the revocation paragraph with:

```markdown
**Fail-open on Redis outage, never fail-closed.** `CedarAuthorizer::is_authorized` reads a
Redis-backed entity-generation counter to build its decision-cache key. If that read errors, the
decision cache is **bypassed entirely** for that call — no key, no lookup, no population — and the
decision is computed **directly** against the in-memory policy snapshot, which is compiled from
**Postgres**. Availability is preserved — every decision still returns — and the bypass is
observable as `iam_authz_decisions_total{cache="bypass"}`. It is **not free**: a decision performs
several counter reads, and with Redis unreachable each pays the client's unbounded reconnect-retry
budget, so **an outage costs roughly 20–30 seconds per decision** (measured on the SMA-470
acceptance test; tracked as **SMA-473**, bound the retry budget). Do not describe this as "latency
only" — at that magnitude most callers time out.

A fail-**closed** posture (denying everything during a Redis outage) is **not offered, by
decision** (SMA-470). Redis is a pure accelerator here — the authoritative policy set never lives
in it — so denying during its outage would convert a latency degradation into a total outage. That
reasoning holds only while the degradation stays small, which is what makes SMA-473 the right
response rather than a reversal of the decision. The contract is bounded-staleness fail-open, and
the bound below is what makes that defensible.

**Revocation freshness is TTL-bounded, and the generation bump is best-effort.** A grant/revoke
bumps `policy_gen` **after** its Postgres transaction commits, and that bump is **logged and
swallowed on failure** (`GenerationsPolicyGenBumper::bump`). With Redis unavailable the revoke
therefore commits while its invalidation signal is lost — the bump is an accelerator, never the
guarantee.

The guarantee is the policy snapshot's **unconditional TTL backstop**: every
`authz.policy_cache_ttl_secs` (default `30`), checked once per `authz.refresh_interval_secs`
(default `1`), the snapshot recompiles from Postgres and installs the result regardless of whether
the generation counter moved. Worst-case revocation latency is therefore
**`policy_cache_ttl_secs + refresh_interval_secs`** — a genuine *sum*, because `spawn_reload`
sleeps `poll` **before** checking the TTL. `IamConfig::validate` caps NEITHER key (it only rejects
`refresh_interval_secs` *greater* than `policy_cache_ttl_secs`; equal is permitted), so there is no
"permitted maximum" to quote: 31s is the bound at the defaults, and the 60s figure is simply
2 × the *default* TTL, reached when an operator raises the poll interval to equal it (design §7a
amendment B). That figure also has to add the reload's own duration on top, and the whole bound
**assumes Postgres is reachable** throughout. A persistently failing load or a malformed policy row
aborts every reload and keeps the last-known-good snapshot indefinitely; that is what
`IamPolicySnapshotReloadsStalled` and `iam_authz_policy_snapshot_reloads_total{outcome="failed"}`
exist to surface.

The decision cache does **not** add to that bound: its key is derived from a content hash of the
compiled policy set (SMA-470), so a reload that picks up a revoke moves every affected request to
a fresh key space immediately. Note that decision-cache **`Allow` hits are not re-audited**
(denials are), so any staleness window is also an audit gap.

**This bound covers role-grant and policy revocation only.** Access changes driven by *tenancy*
state — an organization archived, a membership removed — flow through `entity_gen` and the
entity/slice cache instead, and remain bounded by `authz.slice_cache_ttl_secs` (default `60`) plus
`authz.decision_cache_ttl_secs` (default `30`). **That `entity_gen` bound is Redis-backend-only**
(design §7a amendment B): on `authz.cache.backend = memory` there is no decision-cache TTL and no
slice cache is wired at all, so neither TTL applies and the numbers above must not be quoted for it.

**Redis `maxmemory-policy` must be `volatile-*`, never `allkeys-*`.** `iam:authz:policy_gen` and
`iam:authz:entity_gen` are written without a TTL, so under `allkeys-lru`/`allkeys-random` they are
ordinary eviction candidates. `Generations::read` maps a missing key to `0`, so evicting them
silently rewinds the counters. The snapshot recovers (it reloads on generation *inequality*, not
just advance), but an `allkeys-*` policy turns a routine memory-pressure event into an authz
freshness event for no benefit.
```

- [ ] **Step 2: Correct `policy_snapshot.rs`'s module docs**

Update the **Reload triggers** paragraph to say `reload_if_stale` recompiles when the store's
`policy_gen` **differs from** (not "has advanced past") the compiled `r#gen`, and note the
provisional-stamp suppression. Replace the **Monotonic-write guard** paragraph to describe the
`load_seq` ordering token instead of `r#gen`, explaining that a Redis-sourced counter cannot order
two loads and that requiring it to strictly advance is what disabled the TTL backstop. Add a short
paragraph documenting the fallback stamp and `stamp_trusted`.

- [ ] **Step 3: Verify the docs build and nothing else drifted**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo doc -p paigasus-iam --no-deps 2>&1 | tail -20
```

Expected: no broken intra-doc links (`install_if_newer` no longer exists — every `[`…`]` reference to it must be updated).

- [ ] **Step 4: Commit**

```bash
git add docs/ops/RUNBOOK-observability.md rs/crates/services/paigasus-iam/src/adapters/authz/policy_snapshot.rs
git commit -m "docs(rs): correct the authz availability posture and snapshot module docs"
```

---

### Task 9: Full CI gate run

Per-project Moon tasks do not run the repo-level gates. This task runs the graph the way CI does.

**Files:** none (may produce fixes in files touched by Tasks 1-8).

- [ ] **Step 1: Format and lint**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
```

Fix every warning. `cargo fmt` may reformat the new code — that is expected.

- [ ] **Step 2: Full test run**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass --workspace
```

Expected: PASS. Docker-gated tests skip locally without a daemon; if Docker is available they must pass.

- [ ] **Step 3: Run the full CI graph**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :promtool :observability-drift \
  :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```

Expected: all green. If Moon reports an unattributed failure, diagnose with:

```bash
jq '.actions[]|select(.status=="failed")' .moon/cache/ciReport.json
```

Likely follow-ups: `blake3` added to `paigasus-iam-core` may need a `rs/deny.toml` `[licenses]`
exception (it is already in the tree via `paigasus-iam`, so most likely not); `cargo-machete` is
satisfied because Task 1 consumes the dependency in the same commit.

- [ ] **Step 4: Commit any gate fixes**

```bash
git add -A
git commit -m "chore(rs): satisfy ci gates for SMA-470"
```

Skip this step if the gates were green with no changes.

---

## Self-Review

**Spec coverage:**

| Spec item | Task |
|---|---|
| D4 content-hash cache key | 1, 4 |
| D-B install guard / TTL backstop | 2, 5 |
| D-A unreadable counter tolerance | 3 |
| D-C generation reset recovery | 3 |
| §3.4 single-flight + provisional suppression | 3 |
| D5 telemetry | 6 |
| D1 recorded, RUNBOOK rewrite, `maxmemory-policy`, `entity_gen` scoping, ALLOW-hits-unaudited | 8 |
| §4.3 acceptance test | 7 |
| §9 AC8 full CI graph | 9 |
| D6 (boot behavior) | intentionally none — out of scope, follow-up |
| §8 follow-up issues | filed post-merge, not a code task |

**Type consistency:** `content_hash: String` (Task 1) is consumed as `&compiled.content_hash` →
`decision_key(policy_content: &str, ...)` (Task 4). `load_and_compile` stays an associated fn
throughout: `(policies, grants, &load_seq) -> (CompiledPolicies, u64)` in Task 2, widening to
`(policies, grants, &load_seq, fallback_gen) -> (CompiledPolicies, u64, bool)` in Task 3 — Task 3
Step 3 explicitly updates every call site. `install_if_fresher` likewise gains its third parameter
in Task 3. Task 2 introduces no parameter it does not use, so each task compiles clean on its own.
`reload_now_for_test` is `#[cfg(test)] pub(crate)`, matching its cross-module test consumer in
Task 4.

**Known gaps deliberately left:** the `entity_gen` counter has the identical missing-key→`0`
defect and is scoped out (documented in Task 8's RUNBOOK text, filed as a follow-up); booting
with Redis down is unchanged (D6).
