// SPDX-License-Identifier: Apache-2.0

//! AC #2 — a Principal/User row round-trips through real Postgres.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker
//! daemon is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note.

use chrono::{SubsecRound, Utc};
use paigasus_iam::adapters::persistence::entities::principal;
use paigasus_iam::adapters::persistence::{Migrator, PgPrincipalRepository};
use paigasus_iam_core::{Email, Principal, PrincipalId, PrincipalKind, PrincipalRepository, PrincipalStatus, RepositoryError, User};
use paigasus_kernel::{Prn, mint_uuid7};
use sea_orm::{Database, DatabaseConnection, EntityTrait};
use sea_orm_migration::MigratorTrait;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::ImageExt;
use testcontainers_modules::testcontainers::runners::AsyncRunner;

/// Starts an ephemeral Postgres container, connects, and runs migrations.
///
/// Returns `None` when Docker is unavailable and `CI` is unset (local skip path). Panics
/// when `CI` is set and Docker is unreachable — Docker must be present in CI.
async fn start_migrated_postgres() -> Option<(ContainerAsync<Postgres>, DatabaseConnection)> {
    let node = match Postgres::default().with_tag("16-alpine").start().await {
        Ok(n) => n,
        Err(e) => {
            if std::env::var_os("CI").is_some() {
                panic!("Docker is required for the round-trip test in CI: {e}");
            }
            eprintln!("skipping round-trip: Docker unavailable ({e})");
            return None;
        }
    };

    let port = node.get_host_port_ipv4(5432).await.unwrap();
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let db = Database::connect(&url).await.unwrap();
    Migrator::up(&db, None).await.unwrap();

    Some((node, db))
}

#[tokio::test]
async fn principal_user_round_trips_through_postgres() {
    let Some((_node, db)) = start_migrated_postgres().await else {
        return;
    };

    // Build a principal with µs-truncated timestamps (matches the SystemClock contract).
    let uuid = mint_uuid7(1_700_000_000_000, [7u8; 10]);
    let id = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", uuid).unwrap());
    let now = Utc::now().trunc_subsecs(6);
    let principal = Principal::new(id.clone(), PrincipalKind::User, PrincipalStatus::Active, now, now);
    let user = User::new(id.clone(), Email::parse("roundtrip@example.com").unwrap(), "Round Trip".into(), Some("en-US".into()), None, now, now);

    let repo = PgPrincipalRepository::new(db);
    repo.create_user(&principal, &user).await.unwrap();

    let (got_p, got_u) = repo.find_user(&id).await.unwrap().expect("row present");
    assert_eq!(got_p, principal);
    assert_eq!(got_u, user);
}

/// AC — the two-insert `create_user` is atomic: a duplicate-email failure on the second
/// insert must roll back the first, leaving no orphaned `principal` row.
#[tokio::test]
async fn create_user_rolls_back_principal_on_duplicate_email() {
    let Some((_node, db)) = start_migrated_postgres().await else {
        return;
    };

    // Kept alongside `repo` so the test can query the `principal` entity directly, bypassing
    // `find_user`'s weaker "either row missing" semantics.
    let db_for_direct_query = db.clone();
    let repo = PgPrincipalRepository::new(db);
    let now = Utc::now().trunc_subsecs(6);

    let first_uuid = mint_uuid7(1_700_000_000_001, [1u8; 10]);
    let first_id = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", first_uuid).unwrap());
    let first_principal = Principal::new(first_id.clone(), PrincipalKind::User, PrincipalStatus::Active, now, now);
    let first_user = User::new(first_id.clone(), Email::parse("dupe@example.com").unwrap(), "First User".into(), None, None, now, now);
    repo.create_user(&first_principal, &first_user).await.unwrap();

    let second_uuid = mint_uuid7(1_700_000_000_002, [2u8; 10]);
    let second_id = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", second_uuid).unwrap());
    let second_principal = Principal::new(second_id.clone(), PrincipalKind::User, PrincipalStatus::Active, now, now);
    // Same email as `first_user` — the `user` insert must fail with a unique-constraint
    // violation, and that failure must roll back the already-inserted `second_principal` row.
    let second_user = User::new(second_id.clone(), Email::parse("dupe@example.com").unwrap(), "Second User".into(), None, None, now, now);

    let result = repo.create_user(&second_principal, &second_user).await;
    assert!(matches!(result, Err(RepositoryError::Conflict(_))), "expected Conflict, got {result:?}");

    // The second principal must NOT be orphaned: the transaction rolled back its insert.
    // `find_user` alone is a weak check (it returns `None` if EITHER row is missing), so also
    // query the `principal` entity directly to prove the row itself is absent.
    let found = repo.find_user(&second_id).await.unwrap();
    assert!(found.is_none(), "second principal was orphaned despite the failed transaction");

    let orphan = principal::Entity::find_by_id(second_id.uuid()).one(&db_for_direct_query).await.unwrap();
    assert!(orphan.is_none(), "principal insert must have rolled back — no orphan row");
}
