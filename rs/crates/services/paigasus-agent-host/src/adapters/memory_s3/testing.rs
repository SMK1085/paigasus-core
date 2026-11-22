// SPDX-License-Identifier: Apache-2.0

//! A scripted in-memory `ObjectStore` for the tests of the S3 memory store.
//!
//! The store keeps each object as bytes and a quoted ETag, as S3 sends it. It honours
//! `PutMode::Create` and `PutMode::Update`. A test can add a key to the next listing, make the
//! next listing fail or stall, or make the deletes do nothing. A hook runs the code of another
//! writer before the first `HEAD`, `PUT` or `DELETE`. A fault makes the nth `PUT` or `DELETE`
//! fail, write and then fail, or stall. The store records each `PUT`, `HEAD` and `DELETE` in the
//! order of the calls. Switches make the store misbehave as a bad S3 store does: it can ignore
//! the put conditions, give a version ID, refuse its own listed ETag, or refuse to hold a key
//! and a key under it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use object_store::path::Path;
use object_store::{
    CopyOptions, Error, GetOptions, GetResult, GetResultPayload, ListResult, MultipartUpload, ObjectMeta, ObjectStore, PutMode, PutMultipartOptions, PutOptions, PutPayload, PutResult, Result,
};

/// How the store makes the ETag of an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EtagMode {
    /// Each write gets the next number, so two writes of the same bytes differ.
    Counter,
    /// The ETag is a hash of the bytes, as AWS does. So two writes of the same bytes are equal.
    ContentHash,
    /// The ETag is the size of the object only, so two contents of one length are equal.
    SizeOnly,
}

/// The moment when a hook runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hook {
    /// Before the next `HEAD` request.
    BeforeFirstHead,
    /// Before the next `PUT` request, before its condition check.
    BeforeFirstPut,
    /// Before the next `DELETE` request.
    BeforeFirstDelete,
}

/// What a fault does to one request.
#[derive(Debug)]
pub enum Fault {
    /// The request fails with this error, and the store does not change.
    Fail(Error),
    /// The store does the request, and then the request fails with this error.
    WriteThenFail(Error),
    /// The request never ends.
    Stall,
}

/// The code of the other writer.
struct HookFn(Box<dyn FnOnce(&ScriptedStore) + Send>);

impl fmt::Debug for HookFn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HookFn")
    }
}

#[derive(Debug, Default)]
struct State {
    objects: BTreeMap<Path, (Vec<u8>, String)>,
    next_etag: u64,
    injected: Vec<Path>,
    fail_list: bool,
    stall_list: bool,
    fail_get: bool,
    ignore_deletes: bool,
    ignore_create: bool,
    ignore_update: bool,
    return_version: bool,
    refuse_listed: bool,
    unquoted_listing: bool,
    listed_etags: BTreeSet<String>,
    flat: bool,
    calls: Vec<String>,
    hooks: Vec<(Hook, HookFn)>,
    puts: usize,
    deletes: usize,
    put_faults: BTreeMap<usize, Fault>,
    delete_faults: BTreeMap<usize, Fault>,
}

/// An in-memory `ObjectStore` with hooks for the tests.
#[derive(Debug)]
pub struct ScriptedStore {
    mode: EtagMode,
    state: Arc<Mutex<State>>,
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().expect("the scripted store lock")
}

fn generic(text: &str) -> Box<dyn std::error::Error + Send + Sync> {
    text.to_owned().into()
}

fn meta(location: &Path, bytes: &[u8], e_tag: &str) -> ObjectMeta {
    ObjectMeta {
        location: location.clone(),
        last_modified: Default::default(),
        size: bytes.len() as u64,
        e_tag: Some(e_tag.to_owned()),
        version: None,
    }
}

impl ScriptedStore {
    /// Make an empty store.
    pub fn new(mode: EtagMode) -> Arc<Self> {
        Arc::new(ScriptedStore {
            mode,
            state: Arc::new(Mutex::new(State::default())),
        })
    }

    fn next_etag(&self, state: &mut State, bytes: &[u8]) -> String {
        match self.mode {
            EtagMode::Counter => {
                state.next_etag += 1;
                format!("\"{}\"", state.next_etag)
            }
            EtagMode::ContentHash => {
                let mut hasher = DefaultHasher::new();
                bytes.hash(&mut hasher);
                format!("\"{:016x}\"", hasher.finish())
            }
            EtagMode::SizeOnly => format!("\"{}\"", bytes.len()),
        }
    }

    /// The `PUT`, `HEAD` and `DELETE` calls so far, in order, as `"<METHOD> <key>"`.
    pub fn calls(&self) -> Vec<String> {
        lock(&self.state).calls.clone()
    }

    /// Write bytes at a key with no check. The key is valid by construction.
    pub fn insert_raw(&self, key: &str, bytes: &[u8]) {
        let mut state = lock(&self.state);
        let etag = self.next_etag(&mut state, bytes);
        state.objects.insert(Path::parse(key).expect("a valid key"), (bytes.to_vec(), etag));
    }

    /// The next `list` also yields this key, whatever the prefix.
    pub fn inject_listed_key(&self, key: &str) {
        lock(&self.state).injected.push(Path::parse(key).expect("a valid key"));
    }

    /// The next `list` yields `Error::InvalidPath`.
    pub fn fail_list_with_invalid_path(&self) {
        lock(&self.state).fail_list = true;
    }

    /// From now on a delete reports success and removes nothing.
    pub fn ignore_deletes(&self) {
        lock(&self.state).ignore_deletes = true;
    }

    /// From now on the store ignores the condition of every `PUT`, as `PutMode::Overwrite`.
    pub fn ignore_conditions(&self) {
        let mut state = lock(&self.state);
        state.ignore_create = true;
        state.ignore_update = true;
    }

    /// From now on the store refuses a `Create` over a present key, but accepts every `Update`.
    pub fn ignore_if_match(&self) {
        lock(&self.state).ignore_update = true;
    }

    /// From now on each `PUT` result holds a version ID, as a versioned bucket gives.
    pub fn return_version_id(&self) {
        lock(&self.state).return_version = true;
    }

    /// From now on the store refuses an `Update` with an ETag that a listing gave.
    pub fn refuse_update_with_listed_etag(&self) {
        lock(&self.state).refuse_listed = true;
    }

    /// From now on a listing gives each ETag without its quotes, as a store with another ETag
    /// form for the listing than for the `GET` and the `HEAD`.
    pub fn list_etags_unquoted(&self) {
        lock(&self.state).unquoted_listing = true;
    }

    /// From now on the store cannot hold a key `K` and a key under `K/` at the same time, as a
    /// store over a file system. A `PUT` that would make both fails.
    pub fn flat_namespace(&self) {
        lock(&self.state).flat = true;
    }

    /// The next `list` never yields an item and never ends.
    pub fn stall_list(&self) {
        lock(&self.state).stall_list = true;
    }

    /// The next `GET` (not a `HEAD`) fails with a generic error.
    pub fn fail_next_get(&self) {
        lock(&self.state).fail_get = true;
    }

    /// Run `f` once, at the moment `hook`. `f` acts as another writer on this store.
    pub fn on(&self, hook: Hook, f: impl FnOnce(&ScriptedStore) + Send + 'static) {
        lock(&self.state).hooks.push((hook, HookFn(Box::new(f))));
    }

    /// Apply `fault` to the `nth` `PUT` request (the first is 1).
    pub fn fault_put(&self, nth: usize, fault: Fault) {
        lock(&self.state).put_faults.insert(nth, fault);
    }

    /// Apply `fault` to the `nth` `DELETE` request (the first is 1).
    pub fn fault_delete(&self, nth: usize, fault: Fault) {
        lock(&self.state).delete_faults.insert(nth, fault);
    }

    /// Run the first hook for `at`, if there is one. The lock is free while the hook runs.
    fn fire(&self, at: Hook) {
        let hook = {
            let mut state = lock(&self.state);
            let found = state.hooks.iter().position(|(h, _)| *h == at);
            found.map(|i| state.hooks.remove(i).1)
        };
        if let Some(HookFn(f)) = hook {
            f(self);
        }
    }

    /// Do a `PUT` with its condition check.
    fn put_now(&self, location: &Path, bytes: Vec<u8>, mode: PutMode) -> Result<PutResult> {
        let mut state = lock(&self.state);
        if state.flat && clashes(&state.objects, location) {
            return Err(Error::Generic {
                store: "ScriptedStore",
                source: generic("Server returned non-2xx status code: 409 Conflict"),
            });
        }
        let mode = match mode {
            PutMode::Create if state.ignore_create => PutMode::Overwrite,
            PutMode::Update(_) if state.ignore_update => PutMode::Overwrite,
            mode => mode,
        };
        match mode {
            PutMode::Overwrite => {}
            PutMode::Create => {
                if state.objects.contains_key(location) {
                    return Err(Error::AlreadyExists {
                        path: location.to_string(),
                        source: generic("exists"),
                    });
                }
            }
            PutMode::Update(want) => {
                let held = state.objects.get(location).map(|(_, etag)| etag.as_str());
                let listed = want.e_tag.as_ref().is_some_and(|e| state.refuse_listed && state.listed_etags.contains(e));
                if held.is_none() || held != want.e_tag.as_deref() || listed {
                    return Err(Error::Precondition {
                        path: location.to_string(),
                        source: generic("etag differs"),
                    });
                }
            }
        }
        let etag = self.next_etag(&mut state, &bytes);
        state.objects.insert(location.clone(), (bytes, etag.clone()));
        Ok(PutResult {
            e_tag: Some(etag),
            version: state.return_version.then(|| "v1".to_owned()),
            extensions: Default::default(),
        })
    }
}

/// True when `location` is a key under a held key, or a held key is under `location`.
fn clashes(objects: &BTreeMap<Path, (Vec<u8>, String)>, location: &Path) -> bool {
    let under = |inner: &Path, outer: &Path| inner.prefix_match(outer).is_some_and(|mut rest| rest.next().is_some());
    objects.keys().any(|held| under(location, held) || under(held, location))
}

impl fmt::Display for ScriptedStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ScriptedStore")
    }
}

#[async_trait]
impl ObjectStore for ScriptedStore {
    async fn put_opts(&self, location: &Path, payload: PutPayload, opts: PutOptions) -> Result<PutResult> {
        let bytes: Vec<u8> = payload.iter().flat_map(|part| part.iter().copied()).collect();
        self.fire(Hook::BeforeFirstPut);
        let fault = {
            let mut state = lock(&self.state);
            state.calls.push(format!("PUT {location}"));
            state.puts += 1;
            let nth = state.puts;
            state.put_faults.remove(&nth)
        };
        let after = match fault {
            None => None,
            Some(Fault::Fail(e)) => return Err(e),
            Some(Fault::WriteThenFail(e)) => Some(e),
            Some(Fault::Stall) => return std::future::pending().await,
        };
        let result = self.put_now(location, bytes, opts.mode)?;
        match after {
            Some(e) => Err(e),
            None => Ok(result),
        }
    }

    async fn put_multipart_opts(&self, _location: &Path, _opts: PutMultipartOptions) -> Result<Box<dyn MultipartUpload>> {
        Err(Error::NotImplemented {
            operation: "put_multipart_opts".to_owned(),
            implementer: "ScriptedStore".to_owned(),
        })
    }

    async fn get_opts(&self, location: &Path, options: GetOptions) -> Result<GetResult> {
        if options.head {
            self.fire(Hook::BeforeFirstHead);
        }
        let (bytes, etag) = {
            let mut state = lock(&self.state);
            if options.head {
                state.calls.push(format!("HEAD {location}"));
            } else if std::mem::take(&mut state.fail_get) {
                return Err(Error::Generic {
                    store: "ScriptedStore",
                    source: generic("the get failed"),
                });
            }
            state.objects.get(location).cloned().ok_or_else(|| Error::NotFound {
                path: location.to_string(),
                source: generic("missing"),
            })?
        };
        let object = meta(location, &bytes, &etag);
        options.check_preconditions(&object)?;
        if options.range.is_some() {
            return Err(Error::NotImplemented {
                operation: "get range".to_owned(),
                implementer: "ScriptedStore".to_owned(),
            });
        }
        let len = bytes.len() as u64;
        let body: Vec<u8> = if options.head { Vec::new() } else { bytes };
        let range = if options.head { 0..0 } else { 0..len };
        let stream = futures::stream::once(futures::future::ready(Ok(PutPayload::from(body).into())));
        Ok(GetResult {
            payload: GetResultPayload::Stream(stream.boxed()),
            meta: object,
            range,
            attributes: Default::default(),
            extensions: Default::default(),
        })
    }

    fn delete_stream(&self, locations: BoxStream<'static, Result<Path>>) -> BoxStream<'static, Result<Path>> {
        let handle = ScriptedStore {
            mode: self.mode,
            state: Arc::clone(&self.state),
        };
        let handle = Arc::new(handle);
        locations
            .then(move |location| {
                let store = Arc::clone(&handle);
                async move {
                    let location = location?;
                    store.fire(Hook::BeforeFirstDelete);
                    let fault = {
                        let mut state = lock(&store.state);
                        state.calls.push(format!("DELETE {location}"));
                        state.deletes += 1;
                        let nth = state.deletes;
                        state.delete_faults.remove(&nth)
                    };
                    let after = match fault {
                        None => None,
                        Some(Fault::Fail(e)) => return Err(e),
                        Some(Fault::WriteThenFail(e)) => Some(e),
                        Some(Fault::Stall) => return std::future::pending().await,
                    };
                    let mut state = lock(&store.state);
                    if !state.ignore_deletes {
                        state.objects.remove(&location);
                    }
                    match after {
                        Some(e) => Err(e),
                        None => Ok(location),
                    }
                }
            })
            .boxed()
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, Result<ObjectMeta>> {
        let root = Path::default();
        let prefix = prefix.unwrap_or(&root);
        let mut state = lock(&self.state);
        if std::mem::take(&mut state.stall_list) {
            return futures::stream::pending().boxed();
        }
        if std::mem::take(&mut state.fail_list) {
            let e = Error::InvalidPath {
                source: object_store::path::Error::EmptySegment { path: "a//b".to_owned() },
            };
            return futures::stream::iter(vec![Err(e)]).boxed();
        }
        let mut items: Vec<Result<ObjectMeta>> = state
            .objects
            .iter()
            .filter(|(key, _)| key.prefix_match(prefix).is_some_and(|mut rest| rest.next().is_some()))
            .map(|(key, (bytes, etag))| {
                let shown = if state.unquoted_listing { etag.trim_matches('"') } else { etag };
                Ok(meta(key, bytes, shown))
            })
            .collect();
        let listed: Vec<String> = items.iter().filter_map(|m| m.as_ref().ok()?.e_tag.clone()).collect();
        state.listed_etags.extend(listed);
        for key in std::mem::take(&mut state.injected) {
            items.push(Ok(meta(&key, &[0; 60], "\"injected\"")));
        }
        futures::stream::iter(items).boxed()
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> Result<ListResult> {
        let root = Path::default();
        let prefix = prefix.unwrap_or(&root);
        let state = lock(&self.state);
        let mut common_prefixes = BTreeSet::new();
        let mut objects = Vec::new();
        for (key, (bytes, etag)) in &state.objects {
            let Some(mut parts) = key.prefix_match(prefix) else { continue };
            let Some(first) = parts.next() else { continue };
            if parts.next().is_some() {
                common_prefixes.insert(prefix.clone().join(first));
            } else {
                objects.push(meta(key, bytes, etag));
            }
        }
        Ok(ListResult {
            common_prefixes: common_prefixes.into_iter().collect(),
            objects,
            extensions: Default::default(),
        })
    }

    async fn copy_opts(&self, _from: &Path, _to: &Path, _options: CopyOptions) -> Result<()> {
        Err(Error::NotImplemented {
            operation: "copy_opts".to_owned(),
            implementer: "ScriptedStore".to_owned(),
        })
    }
}
