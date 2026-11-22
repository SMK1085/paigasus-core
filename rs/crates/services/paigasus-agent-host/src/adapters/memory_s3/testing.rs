// SPDX-License-Identifier: Apache-2.0

//! A scripted in-memory `ObjectStore` for the tests of the S3 memory store.
//!
//! The store keeps each object as bytes and a quoted ETag, as S3 sends it. It honours
//! `PutMode::Create` and `PutMode::Update`. A test can add a key to the next listing, make the
//! next listing fail, or make the deletes do nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
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
}

#[derive(Debug, Default)]
struct State {
    objects: BTreeMap<Path, (Vec<u8>, String)>,
    next_etag: u64,
    injected: Vec<Path>,
    fail_list: bool,
    ignore_deletes: bool,
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

    fn next_etag(&self, state: &mut State) -> String {
        match self.mode {
            EtagMode::Counter => {
                state.next_etag += 1;
                format!("\"{}\"", state.next_etag)
            }
        }
    }

    /// Write bytes at a key with no check. The key is valid by construction.
    pub fn insert_raw(&self, key: &str, bytes: &[u8]) {
        let mut state = lock(&self.state);
        let etag = self.next_etag(&mut state);
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
        let mut state = lock(&self.state);
        match opts.mode {
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
                if held.is_none() || held != want.e_tag.as_deref() {
                    return Err(Error::Precondition {
                        path: location.to_string(),
                        source: generic("etag differs"),
                    });
                }
            }
        }
        let etag = self.next_etag(&mut state);
        state.objects.insert(location.clone(), (bytes, etag.clone()));
        Ok(PutResult {
            e_tag: Some(etag),
            version: None,
            extensions: Default::default(),
        })
    }

    async fn put_multipart_opts(&self, _location: &Path, _opts: PutMultipartOptions) -> Result<Box<dyn MultipartUpload>> {
        Err(Error::NotImplemented {
            operation: "put_multipart_opts".to_owned(),
            implementer: "ScriptedStore".to_owned(),
        })
    }

    async fn get_opts(&self, location: &Path, options: GetOptions) -> Result<GetResult> {
        let (bytes, etag) = lock(&self.state).objects.get(location).cloned().ok_or_else(|| Error::NotFound {
            path: location.to_string(),
            source: generic("missing"),
        })?;
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
        let state = Arc::clone(&self.state);
        locations
            .map(move |location| {
                let location = location?;
                let mut state = lock(&state);
                if !state.ignore_deletes {
                    state.objects.remove(&location);
                }
                Ok(location)
            })
            .boxed()
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, Result<ObjectMeta>> {
        let root = Path::default();
        let prefix = prefix.unwrap_or(&root);
        let mut state = lock(&self.state);
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
            .map(|(key, (bytes, etag))| Ok(meta(key, bytes, etag)))
            .collect();
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
