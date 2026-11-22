// SPDX-License-Identifier: Apache-2.0

//! The memory commit mode, chosen at boot from `memory.backend` (spec 6.12).

use std::sync::Arc;

use paigasus_agent_host_core::ports::MemoryStore;
use sea_orm::DatabaseConnection;

use crate::adapters::persistence::memory::PgMemoryStore;
use crate::config::{MemoryBackend, MemoryConfig};

/// How the memory changes of a run commit. `RunCommitter` matches on it, so it never guesses the
/// backend. SMA-829 adds `External(Arc<dyn MemoryStore>)` for S3. A backend switch does not move
/// data (spec 6.8).
#[derive(Debug, Clone)]
pub enum MemoryCommit {
    /// The memory changes commit in the run transaction.
    InTransaction(PgMemoryStore),
}

impl MemoryCommit {
    /// Build the commit mode that `cfg.backend` names.
    #[must_use]
    pub fn from_config(cfg: &MemoryConfig, db: DatabaseConnection) -> Self {
        match cfg.backend {
            MemoryBackend::Postgres => MemoryCommit::InTransaction(PgMemoryStore::new(db)),
            MemoryBackend::S3 => unreachable!("the S3 memory backend is built by MemoryCommit::build"),
        }
    }

    /// The store for the read and write paths of a run.
    #[must_use]
    pub fn store(&self) -> Arc<dyn MemoryStore> {
        match self {
            MemoryCommit::InTransaction(store) => Arc::new(store.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{MemoryCapsConfig, MemoryConfig};

    #[test]
    fn postgres_backend_builds_the_in_transaction_mode() {
        let cfg = MemoryConfig {
            backend: MemoryBackend::Postgres,
            caps: MemoryCapsConfig {
                max_file_bytes: 1,
                max_store_bytes: 1,
                max_store_files: 1,
                max_loaded_file_bytes: 1,
            },
            s3: None,
        };
        let db = DatabaseConnection::default();
        let commit = MemoryCommit::from_config(&cfg, db);
        assert!(matches!(commit, MemoryCommit::InTransaction(_)));
        let _store: Arc<dyn MemoryStore> = commit.store();
    }
}
