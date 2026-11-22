// SPDX-License-Identifier: Apache-2.0

//! The S3 memory store against real S3-compatible stores (SMA-829, spec §7).
//!
//! Three pinned images:
//!
//! - `rustfs/rustfs:1.0.6` is the primary image. Its tests always run.
//! - `versity/versitygw:v1.0.17` ignores the write conditions. The boot probe must refuse it.
//! - `versity/versitygw:v1.11.0` is the fallback image. Its test runs only when
//!   `PAIGASUS_S3_FALLBACK=1` is set. If rustfs breaks, replace `start_rustfs()` with
//!   `start_versitygw(VERSITYGW_FALLBACK.1)` in the rustfs tests. The raw-key tests use the rustfs
//!   CLI `rc`, so on the fallback image they need another raw-key tool.
//!
//! Each container starts through `paigasus_test_docker::start_or_skip`. Run the tests with
//! `PAIGASUS_REQUIRE_DOCKER=1`, so that a skip is a failure. The credentials reach the client
//! only through the injected environment of `build_store`, never through `set_var`.
//!
//! Measured by hand on 2026-11-22 (Docker Desktop, macOS):
//!
//! - rustfs: `docker run --rm -d -p 9000 -e RUSTFS_ACCESS_KEY=<key> -e RUSTFS_SECRET_KEY=<secret>
//!   rustfs/rustfs:1.0.6`. Readiness: `docker exec <c> rc alias set local http://127.0.0.1:9000
//!   <key> <secret>`, then `docker exec <c> rc bucket list local` exits 0 (ready in about 1 s).
//!   The bucket: `rc bucket create local/<bucket>`.
//! - rustfs raw keys: `rc pipe local/<bucket>/<scope>/notes/ < /dev/null` writes the folder marker
//!   `…/notes/` unchanged (exit 0). `rc pipe local/<bucket>/<scope>/a//b` fails with exit 3: the
//!   store answers 400 `InvalidArgument`. A signed `curl --path-as-is --aws-sigv4 … -X PUT
//!   …/a//b` gets the same 400 with the message `Invalid object key`. So rustfs refuses a key
//!   with an empty segment.
//! - versitygw: `docker run --rm -d -p 7070 -e ROOT_ACCESS_KEY=<key> -e ROOT_SECRET_KEY=<secret>
//!   --entrypoint /bin/sh versity/versitygw:<tag> -c 'mkdir -p /gw && exec <binary> --port :7070
//!   posix /gw'`. The binary is `/app/versitygw` in `v1.0.17` and `/usr/local/bin/versitygw` in
//!   `v1.11.0`. A signed `ListBuckets` gave 200 at the first try. A `docker exec <c> mkdir
//!   /gw/<bucket>` makes a bucket (a signed `ListObjectsV2` of it gave 200). The tests make the
//!   bucket in the start command instead, so the bucket exists before the first request.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::{StreamExt, TryStreamExt};
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload};
use paigasus_agent_host::adapters::memory_commit::{MemoryBootError, MemoryCommit};
use paigasus_agent_host::adapters::memory_s3::S3MemoryStore;
use paigasus_agent_host::adapters::memory_s3::client::build_store;
use paigasus_agent_host::adapters::memory_s3::keys::{KeyPrefix, file_path, scope_path};
use paigasus_agent_host::adapters::memory_s3::probe;
use paigasus_agent_host::config::{MemoryBackend, MemoryCapsConfig, MemoryConfig, MemoryS3Config};
use paigasus_agent_host_core::memory::conformance::{self, fresh_scope};
use paigasus_agent_host_core::memory::{Expected, MemoryCaps, MemoryChange, MemoryChangeSet, MemoryCommitSet, MemoryDir, MemoryPath, MemoryScope};
use paigasus_agent_host_core::ports::{MemoryStore, MemoryStoreError};
use sea_orm::DatabaseConnection;
use testcontainers::core::{CmdWaitFor, ExecCommand, IntoContainerPort};
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use uuid::Uuid;

/// The primary image (Sven chose it at Gate 1).
const RUSTFS: (&str, &str) = ("rustfs/rustfs", "1.0.6"); // sha256:ee717072290bec9b11abfc0365f8310b00df1cd9b3639943edae47bdef05044a
/// The negative image: it ignores `If-None-Match` and `If-Match`.
const VERSITYGW_NEGATIVE: (&str, &str) = ("versity/versitygw", "v1.0.17"); // sha256:a13b9adbda0ea9d4b1d929e4bed327e4eccc18d13c23f7e5d192d987dde5651d
/// The fallback image.
const VERSITYGW_FALLBACK: (&str, &str) = ("versity/versitygw", "v1.11.0"); // sha256:8319a530d08d537426e2f855bae5280709795dc73a3f0c347973ef915d6343c5

/// The test credentials. They are not secret: each container is local and short-lived.
const ACCESS_KEY: &str = "paigasus-test";
const SECRET_KEY: &str = "paigasus-test-secret";
/// The bucket that each test makes.
const BUCKET: &str = "paigasus-mem";
/// The time limit for a store to become ready.
const READY_BUDGET: Duration = Duration::from_secs(30);
/// The S3 port of rustfs.
const RUSTFS_PORT: u16 = 9000;
/// The S3 port of versitygw (the `--port :7070` flag).
const VERSITYGW_PORT: u16 = 7070;

/// A running S3-compatible store with one bucket.
struct S3Node {
    node: ContainerAsync<GenericImage>,
    endpoint: String,
    bucket: String,
}

/// Run a command in the container and wait for its end. Returns the exit code and stdout.
async fn exec(node: &ContainerAsync<GenericImage>, cmd: &[&str]) -> (Option<i64>, String) {
    let mut result = node
        .exec(ExecCommand::new(cmd.iter().copied()).with_cmd_ready_condition(CmdWaitFor::exit()))
        .await
        .expect("docker exec");
    let stdout = result.stdout_to_vec().await.expect("exec stdout");
    (result.exit_code().await.expect("exec exit code"), String::from_utf8_lossy(&stdout).into_owned())
}

/// Start rustfs and make the bucket.
///
/// Start (measured 2026-11-22): the image exposes `9000/tcp`, has the volume `/data`, runs as
/// the user `rustfs`, and reads `RUSTFS_ACCESS_KEY` and `RUSTFS_SECRET_KEY`. Readiness is a signed
/// `ListBuckets` from inside the container with the CLI `/usr/bin/rc`: `rc alias set local
/// http://127.0.0.1:9000 <key> <secret>`, then `rc bucket list local` exits 0. The loop is bounded
/// by 30 s. By hand, rustfs was ready in about 1 s. The bucket is `rc bucket create local/<bucket>`.
async fn start_rustfs() -> Option<S3Node> {
    let image = GenericImage::new(RUSTFS.0, RUSTFS.1)
        .with_exposed_port(RUSTFS_PORT.tcp())
        .with_env_var("RUSTFS_ACCESS_KEY", ACCESS_KEY)
        .with_env_var("RUSTFS_SECRET_KEY", SECRET_KEY);
    let node = paigasus_test_docker::start_or_skip(image, "memory_s3 rustfs").await?;
    let port = paigasus_test_docker::mapped_port(&node, RUSTFS_PORT, "rustfs").await;
    let deadline = Instant::now() + READY_BUDGET;
    loop {
        let (alias, _) = exec(&node, &["rc", "alias", "set", "local", "http://127.0.0.1:9000", ACCESS_KEY, SECRET_KEY]).await;
        if alias == Some(0) && exec(&node, &["rc", "bucket", "list", "local"]).await.0 == Some(0) {
            break;
        }
        assert!(Instant::now() < deadline, "rustfs was not ready within {READY_BUDGET:?}");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let (created, out) = exec(&node, &["rc", "bucket", "create", &format!("local/{BUCKET}")]).await;
    assert_eq!(created, Some(0), "rc bucket create: {out}");
    Some(S3Node {
        node,
        endpoint: format!("http://127.0.0.1:{port}"),
        bucket: BUCKET.to_owned(),
    })
}

/// Start versitygw with the posix backend and one bucket.
///
/// Start (measured 2026-11-22): the entrypoint is `/bin/sh`, and the command is `mkdir -p
/// /gw/<bucket> && exec <binary> --port :7070 posix /gw`, with `ROOT_ACCESS_KEY` and
/// `ROOT_SECRET_KEY`. The directory is inside the container (for xattrs). A directory under the
/// posix root is a bucket, so the start command makes the bucket. (A `docker exec mkdir` after
/// the start also makes one.) The binary is `/app/versitygw` in `v1.0.17` and
/// `/usr/local/bin/versitygw` in `v1.11.0`. Readiness is the first successful `list` through
/// `build_store`, bounded by 30 s. By hand, the first signed request succeeded at once.
async fn start_versitygw(tag: &str) -> Option<S3Node> {
    let binary = if tag == VERSITYGW_NEGATIVE.1 { "/app/versitygw" } else { "/usr/local/bin/versitygw" };
    let image = GenericImage::new(VERSITYGW_NEGATIVE.0, tag)
        .with_exposed_port(VERSITYGW_PORT.tcp())
        .with_entrypoint("/bin/sh")
        .with_env_var("ROOT_ACCESS_KEY", ACCESS_KEY)
        .with_env_var("ROOT_SECRET_KEY", SECRET_KEY)
        .with_cmd(["-c".to_owned(), format!("mkdir -p /gw/{BUCKET} && exec {binary} --port :{VERSITYGW_PORT} posix /gw")]);
    let node = paigasus_test_docker::start_or_skip(image, "memory_s3 versitygw").await?;
    let port = paigasus_test_docker::mapped_port(&node, VERSITYGW_PORT, "versitygw").await;
    let s3 = S3Node {
        node,
        endpoint: format!("http://127.0.0.1:{port}"),
        bucket: BUCKET.to_owned(),
    };
    let store = build_store(&s3_config(&s3), env(&s3)).expect("build the client");
    let deadline = Instant::now() + READY_BUDGET;
    while store.list(None).try_next().await.is_err() {
        assert!(Instant::now() < deadline, "versitygw {tag} was not ready within {READY_BUDGET:?}");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Some(s3)
}

/// The injected environment of the client: the two credential variables of the allowlist.
fn env(_node: &S3Node) -> Vec<(String, String)> {
    vec![("AWS_ACCESS_KEY_ID".to_owned(), ACCESS_KEY.to_owned()), ("AWS_SECRET_ACCESS_KEY".to_owned(), SECRET_KEY.to_owned())]
}

/// The S3 section for the node, with `allow_http` and a prefix that is unique for each call.
fn s3_config(node: &S3Node) -> MemoryS3Config {
    MemoryS3Config {
        bucket: node.bucket.clone(),
        prefix: format!("t{}", Uuid::new_v4().simple()),
        region: "us-east-1".to_owned(),
        endpoint: Some(node.endpoint.clone()),
        path_style: None,
        allow_http: true,
        extra_ca_bundle_path: None,
        apply_deadline_secs: 10,
    }
}

/// The raw client, the adapter over it, and the prefix of both.
fn adapter(node: &S3Node) -> (Arc<dyn ObjectStore>, S3MemoryStore, KeyPrefix) {
    let cfg = s3_config(node);
    let raw = build_store(&cfg, env(node)).expect("build the client");
    let prefix = cfg.prefix();
    let store = S3MemoryStore::new(Arc::clone(&raw), prefix.clone(), Duration::from_secs(cfg.apply_deadline_secs));
    (raw, store, prefix)
}

fn p(raw: &str) -> MemoryPath {
    MemoryPath::parse(raw).expect("a valid test path")
}

fn commit_set(scope: &MemoryScope, changes: Vec<MemoryChange>) -> MemoryCommitSet {
    let mut cs = MemoryChangeSet::new();
    for c in changes {
        cs.record(c);
    }
    MemoryCommitSet::new(scope.clone(), cs)
}

fn put_new(path: &str, content: &str) -> MemoryChange {
    MemoryChange::Put {
        path: p(path),
        content: content.to_owned(),
        expected: Expected::Absent,
    }
}

/// Every key under `base`, through the raw client.
async fn raw_keys(raw: &dyn ObjectStore, base: &Path) -> Vec<Path> {
    raw.list(Some(base)).map_ok(|meta| meta.location).try_collect().await.expect("raw listing")
}

/// Write one raw object with the store's own CLI, not through `object_store`. The key goes
/// unchanged to the store, also with a trailing `/` or an empty segment.
async fn rc_pipe_empty(node: &S3Node, key: &str) -> (Option<i64>, String) {
    exec(&node.node, &["sh", "-c", "rc pipe \"$1\" < /dev/null 2>&1", "sh", &format!("local/{}/{key}", node.bucket)]).await
}

/// The recursive listing of the store's own CLI under a key prefix.
async fn rc_list(node: &S3Node, prefix: &str) -> String {
    let (code, out) = exec(&node.node, &["rc", "object", "list", "--recursive", &format!("local/{}/{prefix}/", node.bucket)]).await;
    assert_eq!(code, Some(0), "rc object list: {out}");
    out
}

#[tokio::test]
async fn rustfs_passes_the_conformance_suite() {
    let Some(node) = start_rustfs().await else { return };
    let (_raw, store, _prefix) = adapter(&node);
    conformance::run_all(&store).await;
}

#[tokio::test]
async fn rustfs_passes_the_boot_probe() {
    let Some(node) = start_rustfs().await else { return };
    let (raw, _store, prefix) = adapter(&node);
    let report = probe::run(raw.as_ref(), &prefix, &node.endpoint, &node.bucket).await.expect("the boot probe passes");
    // Step 7, measured on 2026-11-22: rustfs 1.0.6 CAN hold `K` and `K/x` at the same time. A
    // file system store was expected to give `false`; rustfs has a flat key namespace here. So
    // the probe logs no `warn`.
    assert!(report.file_and_dir_of_one_name, "step 7 changed: {report:?}");
}

#[tokio::test]
async fn rustfs_stores_the_envelope_under_the_scope_key() {
    let Some(node) = start_rustfs().await else { return };
    let (raw, store, prefix) = adapter(&node);
    let scope = fresh_scope();
    store.apply(&commit_set(&scope, vec![put_new("/tasks/t1.md", "hello")]), &MemoryCaps::default()).await.expect("create");
    let key = Path::from(format!("{}/{}/{}/{}/tasks/t1.md", prefix.as_str(), scope.org(), scope.agent().uuid(), scope.principal().uuid()));
    let bytes = raw.get(&key).await.expect("raw get").bytes().await.expect("raw bytes");
    assert!(bytes.starts_with(b"paigasus-memory/1 "), "the object does not start with the header");
    assert!(bytes.ends_with(b"\nhello"), "the object does not end with the content");
}

#[tokio::test]
async fn rustfs_version_is_equal_for_list_get_and_head() {
    let Some(node) = start_rustfs().await else { return };
    let (raw, store, prefix) = adapter(&node);
    let scope = fresh_scope();
    store.apply(&commit_set(&scope, vec![put_new("/v.md", "one")]), &MemoryCaps::default()).await.expect("create");
    let key = file_path(&prefix, &scope, &p("/v.md"));
    let listed: Vec<_> = raw.list(Some(&scope_path(&prefix, &scope))).try_collect().await.expect("raw listing");
    assert_eq!(listed.len(), 1);
    let listed = listed[0].e_tag.clone().expect("a listed ETag");
    let got = raw.get(&key).await.expect("raw get").meta.e_tag.expect("a GET ETag");
    let headed = raw.head(&key).await.expect("raw head").e_tag.expect("a HEAD ETag");
    eprintln!("ETag forms: list={listed} get={got} head={headed}");
    // Measured on 2026-11-22: rustfs gives one form for all three, a quoted 32-digit hex string
    // (for example `"711ec46a35b29eb55bcec2e5badaf8be"`). So `version_of` needs no normal form.
    assert_eq!(listed, got, "list and GET ETag differ");
    assert_eq!(got, headed, "GET and HEAD ETag differ");
    // The adapter versions are equal too.
    let entry_version = store.list(&scope, &MemoryDir::root()).await.expect("list")[0].version.clone();
    let file_version = store.get(&scope, &p("/v.md")).await.expect("get").expect("present").version;
    assert_eq!(entry_version, file_version);
}

/// R4-marker (progress.md): `object_store` strips a trailing `/`, so the adapter cannot address
/// a raw folder marker `…/notes/`. `delete_scope` deletes `…/notes` instead, and the marker
/// stays. AG9 then requires that `delete_scope` does not return `Ok`.
///
/// Measured on 2026-11-22 (rustfs 1.0.6): `list` skips the marker; the marker remains after
/// `delete_scope`; `delete_scope` returns `Store("keys remain after the scope delete")`.
#[tokio::test]
async fn rustfs_folder_marker_is_skipped_and_not_erased() {
    let Some(node) = start_rustfs().await else { return };
    let (raw, store, prefix) = adapter(&node);
    let scope = fresh_scope();
    let base = scope_path(&prefix, &scope);
    let (code, out) = rc_pipe_empty(&node, &format!("{base}/notes/")).await;
    assert_eq!(code, Some(0), "rc pipe of the folder marker: {out}");
    assert!(rc_list(&node, base.as_ref()).await.contains("notes/"), "the marker was not written");

    assert_eq!(store.list(&scope, &MemoryDir::root()).await.expect("list"), Vec::new());

    let result = store.delete_scope(&scope).await;
    let remains = rc_list(&node, base.as_ref()).await.contains("notes/");
    eprintln!("folder marker: remains={remains} delete_scope={result:?}");
    assert!(remains, "the marker is gone: the R4-marker ruling no longer matches rustfs");
    let err = result.expect_err("delete_scope must not return Ok while the marker remains (AG9)");
    assert_eq!(err, MemoryStoreError::Store("keys remain after the scope delete".into()));
    assert_eq!(raw_keys(raw.as_ref(), &base).await.len(), 1);
}

/// Measured on 2026-11-22: rustfs 1.0.6 refuses a key with an empty segment (`…/a//b`): the
/// CLI exits non-zero, and the store answers 400 `InvalidArgument` (`Invalid object key`). So
/// such a key cannot exist on rustfs, and the scope stays readable. The `Store` error of spec
/// §6.2 for an unparsable key is covered over the scripted store in the unit tests.
#[tokio::test]
async fn rustfs_refuses_an_empty_segment_key() {
    let Some(node) = start_rustfs().await else { return };
    let (_raw, store, prefix) = adapter(&node);
    let scope = fresh_scope();
    let base = scope_path(&prefix, &scope);
    let (code, out) = rc_pipe_empty(&node, &format!("{base}/a//b")).await;
    assert_ne!(code, Some(0), "rustfs accepted the key a//b: {out}");
    assert!(out.contains("InvalidArgument"), "another refusal: {out}");
    assert_eq!(store.list(&scope, &MemoryDir::root()).await.expect("list"), Vec::new());
}

/// Ruling P15: `[Put /notes/x Absent, Delete /notes v]` with `/notes` present. On a store that
/// cannot hold a file and a directory of one name, the `Put` fails, the `Delete` is not sent, and
/// the error is bare (not `Partial`). On a store with a flat key namespace the set succeeds.
///
/// Measured on 2026-11-22 (rustfs 1.0.6): the set gives `Ok(())`. rustfs has a flat key
/// namespace for this case, the same as the probe step 7 result (`file_and_dir_of_one_name ==
/// true`). So the P15 premise (a bare error) does not hold on rustfs, and the test asserts the
/// measured result: `/notes` is deleted and `/notes/x` is present.
#[tokio::test]
async fn rustfs_file_and_directory_of_one_name() {
    let Some(node) = start_rustfs().await else { return };
    let (_raw, store, _prefix) = adapter(&node);
    let scope = fresh_scope();
    store.apply(&commit_set(&scope, vec![put_new("/notes", "file")]), &MemoryCaps::default()).await.expect("create /notes");
    let v = store.get(&scope, &p("/notes")).await.expect("get").expect("present").version;
    let set = commit_set(&scope, vec![put_new("/notes/x", "child"), MemoryChange::Delete { path: p("/notes"), expected: v }]);
    let result = store.apply(&set, &MemoryCaps::default()).await;
    eprintln!("file and directory of one name: {result:?}");
    assert_eq!(result, Ok(()), "rustfs no longer holds a file and a directory of one name");
    assert!(store.get(&scope, &p("/notes")).await.expect("get").is_none(), "/notes is still present");
    let child = store.get(&scope, &p("/notes/x")).await.expect("get").expect("/notes/x is present");
    assert_eq!(child.content, "child");
}

/// Review Focus 3: a scope with more than one listing page (1,000 keys per page).
#[tokio::test]
async fn rustfs_delete_scope_removes_more_than_one_listing_page() {
    let Some(node) = start_rustfs().await else { return };
    let (raw, store, prefix) = adapter(&node);
    let scope = fresh_scope();
    let base = scope_path(&prefix, &scope);
    let keys: Vec<Path> = (0..1001).map(|i| file_path(&prefix, &scope, &p(&format!("/bulk/f{i:04}.md")))).collect();
    futures::stream::iter(keys)
        .map(|key| {
            let raw = Arc::clone(&raw);
            async move { raw.put(&key, PutPayload::from_static(b"x")).await }
        })
        .buffer_unordered(16)
        .try_collect::<Vec<_>>()
        .await
        .expect("raw puts");
    assert_eq!(raw_keys(raw.as_ref(), &base).await.len(), 1001);
    store.delete_scope(&scope).await.expect("delete_scope");
    assert_eq!(raw_keys(raw.as_ref(), &base).await, Vec::<Path>::new());
}

/// The real boot path: `MemoryCommit::build` with `backend = s3` runs the probe, and the probe
/// refuses a store that accepts a second create-only put.
#[tokio::test]
async fn versitygw_1_0_17_fails_the_boot_probe_at_step_2() {
    let Some(node) = start_versitygw(VERSITYGW_NEGATIVE.1).await else { return };
    let cfg = MemoryConfig {
        backend: MemoryBackend::S3,
        caps: MemoryCapsConfig {
            max_file_bytes: 1,
            max_store_bytes: 1,
            max_store_files: 1,
            max_loaded_file_bytes: 1,
        },
        s3: Some(s3_config(&node)),
    };
    let result = MemoryCommit::build(&cfg, DatabaseConnection::default(), env(&node)).await;
    let Err(MemoryBootError::Probe(e)) = result else {
        panic!("expected a probe error, got {result:?}");
    };
    assert_eq!(e.step, 2, "{e}");
    assert!(e.to_string().contains("does not support conditional writes"), "{e}");
}

#[tokio::test]
async fn versitygw_fallback_passes_conformance_and_probe() {
    if std::env::var_os("PAIGASUS_S3_FALLBACK").as_deref() != Some(std::ffi::OsStr::new("1")) {
        eprintln!("SKIP[s3-fallback] set PAIGASUS_S3_FALLBACK=1 to run");
        return;
    }
    let Some(node) = start_versitygw(VERSITYGW_FALLBACK.1).await else { return };
    let (raw, store, prefix) = adapter(&node);
    conformance::run_all(&store).await;
    let report = probe::run(raw.as_ref(), &prefix, &node.endpoint, &node.bucket).await.expect("the boot probe passes");
    eprintln!("versitygw {} probe: {report:?}", VERSITYGW_FALLBACK.1);
}
