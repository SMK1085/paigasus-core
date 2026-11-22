// SPDX-License-Identifier: Apache-2.0

//! The memory commit mode, chosen at boot from `memory.backend` (spec 6.12).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use paigasus_agent_host_core::ports::MemoryStore;
use sea_orm::DatabaseConnection;

use crate::adapters::memory_s3::S3MemoryStore;
use crate::adapters::memory_s3::client::{ClientError, build_store};
use crate::adapters::memory_s3::probe::{self, ProbeError};
use crate::adapters::persistence::memory::PgMemoryStore;
use crate::config::{MemoryBackend, MemoryConfig};

/// How the memory changes of a run commit. `RunCommitter` matches on it, so it never guesses the
/// backend. A backend switch does not move data (spec 6.8).
#[derive(Clone)]
pub enum MemoryCommit {
    /// The memory changes commit in the run transaction.
    InTransaction(PgMemoryStore),
    /// The memory changes commit in an external store, after the run transaction.
    External(Arc<dyn MemoryStore>),
}

impl fmt::Debug for MemoryCommit {
    /// Print only the variant name. The store can hold an endpoint, so it is not printed.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemoryCommit::InTransaction(_) => f.write_str("InTransaction"),
            MemoryCommit::External(_) => f.write_str("External"),
        }
    }
}

/// The commit mode cannot be built at boot.
#[derive(Debug, thiserror::Error)]
pub enum MemoryBootError {
    /// `memory.backend` is `s3`, and there is no `[memory.s3]` section.
    #[error("memory.backend is \"s3\", but the [memory.s3] section is missing")]
    MissingS3Section,
    /// The S3 client cannot be built.
    #[error(transparent)]
    Client(#[from] ClientError),
    /// The boot probe of the S3 store failed.
    #[error(transparent)]
    Probe(#[from] ProbeError),
}

impl MemoryCommit {
    /// Build the commit mode that `cfg.backend` names. For `s3` it builds the client from `env`,
    /// runs the boot probe, and keeps the store.
    ///
    /// # Errors
    ///
    /// A [`MemoryBootError`] when the S3 section is missing, the client cannot be built, or the
    /// boot probe fails.
    pub async fn build(cfg: &MemoryConfig, db: DatabaseConnection, env: impl IntoIterator<Item = (String, String)>) -> Result<Self, MemoryBootError> {
        match cfg.backend {
            MemoryBackend::Postgres => Ok(MemoryCommit::InTransaction(PgMemoryStore::new(db))),
            MemoryBackend::S3 => {
                let s3 = cfg.s3.as_ref().ok_or(MemoryBootError::MissingS3Section)?;
                let store = build_store(s3, env)?;
                let prefix = s3.prefix();
                let report = probe::run(store.as_ref(), &prefix, s3.endpoint.as_deref().unwrap_or("AWS"), &s3.bucket).await?;
                tracing::info!(
                    bucket = %s3.bucket,
                    file_and_dir_of_one_name = report.file_and_dir_of_one_name,
                    "the S3 memory store passed the boot probe"
                );
                Ok(MemoryCommit::External(Arc::new(S3MemoryStore::new(store, prefix, Duration::from_secs(s3.apply_deadline_secs)))))
            }
        }
    }

    /// The store for the read and write paths of a run.
    #[must_use]
    pub fn store(&self) -> Arc<dyn MemoryStore> {
        match self {
            MemoryCommit::InTransaction(store) => Arc::new(store.clone()),
            MemoryCommit::External(store) => Arc::clone(store),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::memory_s3::keys::KeyPrefix;
    use crate::config::{MemoryCapsConfig, MemoryConfig, MemoryS3Config};
    use object_store::memory::InMemory;

    fn cfg(backend: MemoryBackend, s3: Option<MemoryS3Config>) -> MemoryConfig {
        MemoryConfig {
            backend,
            caps: MemoryCapsConfig {
                max_file_bytes: 1,
                max_store_bytes: 1,
                max_store_files: 1,
                max_loaded_file_bytes: 1,
            },
            s3,
        }
    }

    fn external() -> (Arc<dyn MemoryStore>, MemoryCommit) {
        let store: Arc<dyn MemoryStore> = Arc::new(S3MemoryStore::new(Arc::new(InMemory::new()), KeyPrefix::parse("mem").unwrap(), Duration::from_secs(10)));
        let commit = MemoryCommit::External(Arc::clone(&store));
        (store, commit)
    }

    #[tokio::test]
    async fn postgres_backend_builds_the_in_transaction_mode() {
        let commit = MemoryCommit::build(&cfg(MemoryBackend::Postgres, None), DatabaseConnection::default(), Vec::new()).await.unwrap();
        assert!(matches!(commit, MemoryCommit::InTransaction(_)));
        let _store: Arc<dyn MemoryStore> = commit.store();
    }

    #[tokio::test]
    async fn s3_backend_without_a_section_is_an_error() {
        let result = MemoryCommit::build(&cfg(MemoryBackend::S3, None), DatabaseConnection::default(), Vec::new()).await;
        assert!(matches!(result, Err(MemoryBootError::MissingS3Section)));
    }

    #[test]
    fn the_debug_text_names_only_the_variant() {
        let (_store, commit) = external();
        assert_eq!(format!("{commit:?}"), "External");
        let pg = MemoryCommit::InTransaction(PgMemoryStore::new(DatabaseConnection::default()));
        assert_eq!(format!("{pg:?}"), "InTransaction");
    }

    #[test]
    fn the_external_store_is_the_same_arc() {
        let (inner, commit) = external();
        assert!(Arc::ptr_eq(&commit.store(), &inner));
    }

    #[tokio::test]
    async fn s3_backend_with_a_bad_ca_path_is_a_client_error() {
        let s3: MemoryS3Config = serde_json::from_value(serde_json::json!({"bucket": "paigasus-mem", "prefix": "mem", "extra_ca_bundle_path": "/nonexistent/ca.pem"})).unwrap();
        let result = MemoryCommit::build(&cfg(MemoryBackend::S3, Some(s3)), DatabaseConnection::default(), Vec::new()).await;
        assert!(matches!(result, Err(MemoryBootError::Client(ClientError::CaRead { .. }))), "{result:?}");
    }
}
