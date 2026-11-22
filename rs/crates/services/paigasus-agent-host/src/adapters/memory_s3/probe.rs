// SPDX-License-Identifier: Apache-2.0

//! The boot probe of the S3 memory store (spec §6.8).
//!
//! The probe runs once at boot. It writes, reads and deletes one object under
//! `<prefix>/_probe/<random UUID>`, and it refuses a store that the adapter cannot use:
//!
//! 1. A create-only `PUT`. Its result must hold no version ID, else the bucket keeps old versions.
//! 2. The same create-only `PUT` again. The store must refuse it (`If-None-Match`).
//! 3. A conditional `PUT` of other content of the same length with the ETag of step 1. It must
//!    succeed and give a new ETag.
//! 4. A conditional `PUT` with the stale ETag of step 1. The store must refuse it (`If-Match`).
//! 5. The ETag of the listing, the `GET` and the `HEAD` must be equal to the ETag of step 3. A
//!    conditional `PUT` with the listed ETag must succeed.
//! 6. A `DELETE`, then a `HEAD`. The object must be absent.
//! 7. A create-only `PUT` of `<key>/x` and of `<key>`. If the second `PUT` fails, the store cannot
//!    hold a file and a directory of one name. The probe logs a `warn` and does not fail.
//!
//! The probe proves the behavior of the store once, at boot. A store that changes later is not
//! found. The probe deletes its objects when it ends, also after a failure where it can. No error
//! text holds a key or a path (AG8): it holds the endpoint, the bucket, the step and an error
//! class of `errors`. The endpoint and the bucket are operator config, not principal data.

use std::fmt;

use futures::TryStreamExt;
use object_store::path::Path;
use object_store::{Error, ObjectStore, ObjectStoreExt, PutMode, PutOptions, PutPayload, PutResult, UpdateVersion};
use uuid::Uuid;

use super::errors::{class, is_refusal};
use super::keys::{KeyPrefix, probe_dir, probe_path};
use super::version_of;

/// The content of the first probe write.
const CONTENT_A: &[u8] = b"paigasus memory probe A";
/// The content of the second probe write. It has the length of `CONTENT_A`.
const CONTENT_B: &[u8] = b"paigasus memory probe B";
/// The content of the step 7 writes.
const CONTENT_DIR: &[u8] = b"paigasus memory probe 7";

/// The result of a probe that passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeReport {
    /// True when the store can hold a key `K` and a key under `K/` at the same time (step 7).
    pub file_and_dir_of_one_name: bool,
}

/// Why a probe step failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeFailure {
    /// The result of the first write held a version ID (step 1).
    VersionedBucket,
    /// The store accepted a create-only write over a present object (step 2).
    CreateNotRefused,
    /// Two different contents of one length got the same ETag (step 3).
    EtagRepeats,
    /// The store accepted a conditional write with a stale ETag (step 4).
    StaleUpdateNotRefused,
    /// The listed, the `GET` or the `HEAD` ETag is not the ETag of the last write (step 5).
    EtagFormDiffers,
    /// The store refused a conditional write with its listed ETag (step 5).
    ListedEtagRefused,
    /// The object was present after its delete (step 6).
    DeleteKept,
    /// A request failed. `class` is a fixed class text of `errors`.
    Request {
        /// The error class.
        class: String,
    },
}

/// A failed boot probe. The text names the endpoint, the bucket and the step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeError {
    /// The endpoint of the store, as the config gives it.
    pub endpoint: String,
    /// The bucket.
    pub bucket: String,
    /// The step that failed, 1 to 6.
    pub step: u8,
    /// Why the step failed.
    pub failure: ProbeFailure,
}

impl ProbeError {
    /// True when the failure shows that the store does not do conditional writes: a step 2 or
    /// step 4 write that the store accepted, or a 501 at step 1, 3 or 4.
    fn no_conditional_writes(&self) -> bool {
        match &self.failure {
            ProbeFailure::CreateNotRefused | ProbeFailure::StaleUpdateNotRefused => true,
            ProbeFailure::Request { class } => matches!(self.step, 1 | 3 | 4) && (class == "status=501" || class == "not_implemented"),
            _ => false,
        }
    }
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ProbeError { endpoint, bucket, step, failure } = self;
        if self.no_conditional_writes() {
            let kind = if *step <= 2 { "create-only" } else { "conditional" };
            return write!(
                f,
                "The memory store at {endpoint}, bucket {bucket}, does not support conditional writes: it accepted a {kind} put that it must refuse. The S3 memory backend needs If-None-Match and If-Match on PutObject."
            );
        }
        write!(f, "The memory store at {endpoint}, bucket {bucket}, failed the boot probe at step {step}: ")?;
        match failure {
            ProbeFailure::VersionedBucket => {
                f.write_str("the bucket keeps old versions of objects. The S3 memory backend needs a bucket that does not keep old versions, because a delete in a versioned bucket keeps the data.")
            }
            ProbeFailure::EtagRepeats => f.write_str("two different contents of one length got the same ETag. The S3 memory backend needs an ETag that changes when the content changes."),
            ProbeFailure::EtagFormDiffers => f.write_str("the ETag of the listing, the GET and the HEAD of one object is not the same. The S3 memory backend needs one ETag form for all three."),
            ProbeFailure::ListedEtagRefused => f.write_str("the store refused a conditional put with the ETag from its own listing. The S3 memory backend needs If-Match to accept the listed ETag."),
            ProbeFailure::DeleteKept => f.write_str("the object was still present after a delete. The S3 memory backend needs a working DELETE and the s3:DeleteObject permission."),
            ProbeFailure::Request { class } => write!(f, "{class}."),
            // `no_conditional_writes` is true for these two, so the text is written above.
            ProbeFailure::CreateNotRefused | ProbeFailure::StaleUpdateNotRefused => Ok(()),
        }
    }
}

impl std::error::Error for ProbeError {}

/// The failure of one request, with its error class.
fn request(e: &Error) -> ProbeFailure {
    ProbeFailure::Request { class: class(e) }
}

/// The ETag of a write result. A result with no ETag is a request failure of class `other`.
fn etag_of(result: &PutResult) -> Result<String, ProbeFailure> {
    result.e_tag.clone().ok_or_else(|| ProbeFailure::Request { class: "other".to_owned() })
}

/// One write with a mode.
async fn put(store: &dyn ObjectStore, key: &Path, bytes: &'static [u8], mode: PutMode) -> Result<PutResult, Error> {
    store.put_opts(key, PutPayload::from_static(bytes), PutOptions::from(mode)).await
}

/// The conditional write mode for an ETag.
fn update(e_tag: &str) -> PutMode {
    PutMode::Update(UpdateVersion {
        e_tag: Some(e_tag.to_owned()),
        version: None,
    })
}

/// Run the boot probe. `endpoint` is the config endpoint, or `the AWS default endpoint` when no
/// endpoint is set. A failure of steps 1 to 6 is an error. Step 7 never fails the probe.
pub async fn run(store: &dyn ObjectStore, prefix: &KeyPrefix, endpoint: &str, bucket: &str) -> Result<ProbeReport, ProbeError> {
    let key = probe_path(prefix, Uuid::new_v4());
    match steps(store, prefix, &key).await {
        Ok(()) => Ok(ProbeReport {
            file_and_dir_of_one_name: file_and_dir(store, &key, endpoint).await,
        }),
        Err((step, failure)) => {
            // Best effort: the probe failed already, so a failed delete changes nothing.
            let _ = store.delete(&key).await;
            Err(ProbeError {
                endpoint: endpoint.to_owned(),
                bucket: bucket.to_owned(),
                step,
                failure,
            })
        }
    }
}

/// Steps 1 to 6. A failure gives the step and the failure.
async fn steps(store: &dyn ObjectStore, prefix: &KeyPrefix, key: &Path) -> Result<(), (u8, ProbeFailure)> {
    // Step 1: a create-only write, with no version ID in its result.
    let first = put(store, key, CONTENT_A, PutMode::Create).await.map_err(|e| (1, request(&e)))?;
    if first.version.is_some() {
        return Err((1, ProbeFailure::VersionedBucket));
    }
    let etag_a = etag_of(&first).map_err(|f| (1, f))?;

    // Step 2: the same create-only write must be refused.
    match put(store, key, CONTENT_A, PutMode::Create).await {
        Ok(_) => return Err((2, ProbeFailure::CreateNotRefused)),
        Err(e) if is_refusal(&e) => {}
        Err(e) => return Err((2, request(&e))),
    }

    // Step 3: a conditional write of other content gives a new ETag.
    let second = put(store, key, CONTENT_B, update(&etag_a)).await.map_err(|e| (3, request(&e)))?;
    let etag_b = etag_of(&second).map_err(|f| (3, f))?;
    if etag_b == etag_a {
        return Err((3, ProbeFailure::EtagRepeats));
    }

    // Step 4: a conditional write with the stale ETag must be refused.
    match put(store, key, CONTENT_A, update(&etag_a)).await {
        Ok(_) => return Err((4, ProbeFailure::StaleUpdateNotRefused)),
        Err(e) if is_refusal(&e) => {}
        Err(e) => return Err((4, request(&e))),
    }

    // Step 5.
    listed_etag(store, prefix, key, &etag_b).await.map_err(|f| (5, f))?;

    // Step 6: the delete removes the object.
    store.delete(key).await.map_err(|e| (6, request(&e)))?;
    match store.head(key).await {
        Ok(_) => Err((6, ProbeFailure::DeleteKept)),
        Err(Error::NotFound { .. }) => Ok(()),
        Err(e) => Err((6, request(&e))),
    }
}

/// Step 5: the listing, the `GET` and the `HEAD` give the ETag of step 3 (`etag_b`), and a
/// conditional write with the listed ETag succeeds.
async fn listed_etag(store: &dyn ObjectStore, prefix: &KeyPrefix, key: &Path, etag_b: &str) -> Result<(), ProbeFailure> {
    let mut listing = store.list(Some(&probe_dir(prefix)));
    let mut listed = None;
    while let Some(meta) = listing.try_next().await.map_err(|e| request(&e))? {
        if meta.location == *key {
            listed = Some(meta.e_tag);
        }
    }
    let Some(listed) = listed else {
        return Err(ProbeFailure::Request { class: "not_found".to_owned() });
    };
    let got = store.get(key).await.map_err(|e| request(&e))?.meta.e_tag;
    let headed = store.head(key).await.map_err(|e| request(&e))?.e_tag;
    let want = version_of(Some(etag_b));
    if [&listed, &got, &headed].into_iter().any(|form| version_of(form.as_deref()) != want) {
        return Err(ProbeFailure::EtagFormDiffers);
    }
    let listed = listed.unwrap_or_default();
    match put(store, key, CONTENT_A, update(&listed)).await {
        Ok(_) => Ok(()),
        Err(e) if is_refusal(&e) => Err(ProbeFailure::ListedEtagRefused),
        Err(e) => Err(request(&e)),
    }
}

/// Step 7: write `<key>/x`, then `<key>`, then delete both. False when a write fails. The
/// probe cannot tell a store that refuses the pair from a failed request, so a failure of
/// either write gives false, which is the safe side.
async fn file_and_dir(store: &dyn ObjectStore, key: &Path, endpoint: &str) -> bool {
    let child = key.clone().join("x");
    let held = put(store, &child, CONTENT_DIR, PutMode::Create).await.is_ok() && put(store, key, CONTENT_DIR, PutMode::Create).await.is_ok();
    // Best effort: step 7 never fails the probe, so a failed delete is not an error.
    let _ = store.delete(&child).await;
    let _ = store.delete(key).await;
    if !held {
        tracing::warn!("The memory store at {endpoint} cannot hold a file and a directory of one name. A memory change set that replaces a file with a directory of the same name will fail.");
    }
    held
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::memory_s3::testing::{EtagMode, Fault, ScriptedStore};
    use futures::TryStreamExt;
    use std::sync::Arc;

    const ENDPOINT: &str = "http://store.example:9000";
    const BUCKET: &str = "memory-bucket";

    fn prefix() -> KeyPrefix {
        KeyPrefix::parse("mem").unwrap()
    }

    async fn probe(store: &ScriptedStore) -> Result<ProbeReport, ProbeError> {
        run(store, &prefix(), ENDPOINT, BUCKET).await
    }

    /// The keys under `mem/_probe/` that the probe left in the store.
    async fn probe_keys_left(store: &ScriptedStore) -> Vec<String> {
        let metas: Vec<_> = store.list(Some(&probe_dir(&prefix()))).try_collect().await.unwrap();
        metas.into_iter().map(|m| m.location.to_string()).collect()
    }

    fn generic_status(code: u16) -> Error {
        Error::Generic {
            store: "S3",
            source: format!("Server returned non-2xx status code: {code} Not Implemented: mem/_probe/x").into(),
        }
    }

    /// Each probe failure that the tests below make, with the step and the failure.
    fn failing_stores() -> Vec<(Arc<ScriptedStore>, u8, ProbeFailure)> {
        let ignores_conditions = ScriptedStore::new(EtagMode::Counter);
        ignores_conditions.ignore_conditions();
        let ignores_if_match = ScriptedStore::new(EtagMode::Counter);
        ignores_if_match.ignore_if_match();
        let size_only = ScriptedStore::new(EtagMode::SizeOnly);
        let versioned = ScriptedStore::new(EtagMode::Counter);
        versioned.return_version_id();
        let refuses_listed = ScriptedStore::new(EtagMode::Counter);
        refuses_listed.refuse_update_with_listed_etag();
        let keeps_deletes = ScriptedStore::new(EtagMode::Counter);
        keeps_deletes.ignore_deletes();
        vec![
            (ignores_conditions, 2, ProbeFailure::CreateNotRefused),
            (ignores_if_match, 4, ProbeFailure::StaleUpdateNotRefused),
            (size_only, 3, ProbeFailure::EtagRepeats),
            (versioned, 1, ProbeFailure::VersionedBucket),
            (refuses_listed, 5, ProbeFailure::ListedEtagRefused),
            (keeps_deletes, 6, ProbeFailure::DeleteKept),
        ]
    }

    #[tokio::test]
    async fn a_good_store_passes() {
        let store = ScriptedStore::new(EtagMode::Counter);
        assert_eq!(probe(&store).await, Ok(ProbeReport { file_and_dir_of_one_name: true }));
        assert_eq!(probe_keys_left(&store).await, Vec::<String>::new());
    }

    #[tokio::test]
    async fn ignored_conditions_fail_at_step_2() {
        let store = ScriptedStore::new(EtagMode::Counter);
        store.ignore_conditions();
        let e = probe(&store).await.unwrap_err();
        assert_eq!((e.step, &e.failure), (2, &ProbeFailure::CreateNotRefused));
        let text = e.to_string();
        assert!(text.contains("does not support conditional writes"), "{text}");
        assert!(text.contains("create-only"), "{text}");
        assert_eq!(probe_keys_left(&store).await, Vec::<String>::new(), "the probe cleans up after a failure");
    }

    #[tokio::test]
    async fn a_store_that_ignores_if_match_fails_at_step_4() {
        let store = ScriptedStore::new(EtagMode::Counter);
        store.ignore_if_match();
        let e = probe(&store).await.unwrap_err();
        assert_eq!((e.step, &e.failure), (4, &ProbeFailure::StaleUpdateNotRefused));
        let text = e.to_string();
        assert!(text.contains("conditional put"), "{text}");
        assert!(text.contains("does not support conditional writes"), "{text}");
    }

    #[tokio::test]
    async fn a_size_etag_fails_at_step_3() {
        let store = ScriptedStore::new(EtagMode::SizeOnly);
        let e = probe(&store).await.unwrap_err();
        assert_eq!((e.step, &e.failure), (3, &ProbeFailure::EtagRepeats));
        assert_eq!(probe_keys_left(&store).await, Vec::<String>::new(), "the probe cleans up after a failure");
    }

    #[tokio::test]
    async fn a_versioned_bucket_fails_at_step_1() {
        let store = ScriptedStore::new(EtagMode::Counter);
        store.return_version_id();
        let e = probe(&store).await.unwrap_err();
        assert_eq!((e.step, &e.failure), (1, &ProbeFailure::VersionedBucket));
    }

    #[tokio::test]
    async fn a_refused_listed_etag_fails_at_step_5() {
        let store = ScriptedStore::new(EtagMode::Counter);
        store.refuse_update_with_listed_etag();
        let e = probe(&store).await.unwrap_err();
        assert_eq!((e.step, &e.failure), (5, &ProbeFailure::ListedEtagRefused));
    }

    #[tokio::test]
    async fn a_different_listed_etag_form_fails_at_step_5() {
        let store = ScriptedStore::new(EtagMode::Counter);
        store.list_etags_unquoted();
        let e = probe(&store).await.unwrap_err();
        assert_eq!((e.step, &e.failure), (5, &ProbeFailure::EtagFormDiffers));
        assert!(e.to_string().contains("not the same"), "{e}");
        assert_eq!(probe_keys_left(&store).await, Vec::<String>::new(), "the probe cleans up after a failure");
    }

    #[tokio::test]
    async fn a_delete_that_does_nothing_fails_at_step_6() {
        let store = ScriptedStore::new(EtagMode::Counter);
        store.ignore_deletes();
        let e = probe(&store).await.unwrap_err();
        assert_eq!((e.step, &e.failure), (6, &ProbeFailure::DeleteKept));
    }

    #[tokio::test]
    async fn a_flat_namespace_passes_with_a_warn() {
        let store = ScriptedStore::new(EtagMode::Counter);
        store.flat_namespace();
        assert_eq!(probe(&store).await, Ok(ProbeReport { file_and_dir_of_one_name: false }));
        assert_eq!(probe_keys_left(&store).await, Vec::<String>::new());
    }

    #[tokio::test]
    async fn a_request_error_gives_its_class_or_the_conditional_writes_text() {
        let conditional = "does not support conditional writes";
        let not_implemented = || Error::NotImplemented {
            operation: "mem/_probe/x".to_owned(),
            implementer: "S3".to_owned(),
        };
        let denied = || Error::PermissionDenied {
            path: "mem/_probe/x".to_owned(),
            source: "denied".into(),
        };
        // (the PUT that fails, its error, the step, the class, a text that the message holds)
        let rows: Vec<(usize, Error, u8, &str, &str)> = vec![
            (1, generic_status(501), 1, "status=501", "accepted a create-only put"),
            (1, not_implemented(), 1, "not_implemented", "accepted a create-only put"),
            (3, generic_status(501), 3, "status=501", "accepted a conditional put"),
            (4, not_implemented(), 4, "not_implemented", "accepted a conditional put"),
            (1, denied(), 1, "permission_denied", "failed the boot probe at step 1: permission_denied."),
            (5, generic_status(503), 5, "status=503", "failed the boot probe at step 5: status=503."),
        ];
        for (nth, error, step, class, text) in rows {
            let store = ScriptedStore::new(EtagMode::Counter);
            store.fault_put(nth, Fault::Fail(error));
            let e = probe(&store).await.unwrap_err();
            assert_eq!((e.step, &e.failure), (step, &ProbeFailure::Request { class: class.to_owned() }), "PUT {nth}");
            let message = e.to_string();
            assert!(message.contains(text), "{message}");
            assert_eq!(message.contains(conditional), text.starts_with("accepted"), "{message}");
            assert_eq!(probe_keys_left(&store).await, Vec::<String>::new(), "PUT {nth}");
        }
    }

    #[tokio::test]
    async fn each_probe_error_names_the_endpoint_and_the_bucket() {
        let mut stores = failing_stores();
        let failed = ScriptedStore::new(EtagMode::Counter);
        failed.fault_put(1, Fault::Fail(generic_status(503)));
        stores.push((failed, 1, ProbeFailure::Request { class: "status=503".to_owned() }));
        for (store, step, failure) in stores {
            let e = probe(&store).await.unwrap_err();
            assert_eq!((e.step, &e.failure), (step, &failure));
            assert_eq!((e.endpoint.as_str(), e.bucket.as_str()), (ENDPOINT, BUCKET));
            let text = e.to_string();
            assert!(text.contains(ENDPOINT) && text.contains(BUCKET), "{text}");
            assert!(!text.contains("_probe") && !text.contains("mem/"), "no key: {text}");
        }
    }
}
