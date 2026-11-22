// SPDX-License-Identifier: Apache-2.0

//! The `test-model` feature (SMA-790 §6.11). It exists only for the SIGKILL test of the real
//! binary, and a release build with it does not compile (`lib.rs`).
//!
//! - [`register`] reads a script file and registers a [`ScriptedModel`] as `scripted`. The model
//!   logic lives once, in `adapters::helikon::scripted`. This module only reads the file.
//! - [`FileBarrier`] is a `FaultPoint`: at its stage it creates a file and then waits for ever, so
//!   that the test can kill the process at a known place.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::Context;
use async_trait::async_trait;
use paigasus_agent_host_core::fault::{FaultPoint, FaultStage};

use crate::adapters::helikon::model_registry::{ModelRegistry, SharedModel};
use crate::adapters::helikon::scripted::{ScriptedCall, ScriptedModel};

/// The path of the script file: a JSON array of turns, each turn an array of
/// `["call_id", "tool", {args}]` calls.
pub const SCRIPT_ENV: &str = "PAIGASUS_TEST_MODEL_SCRIPT";
/// The stage of the barrier: `after_run`, `before_commit` or `after_commit`.
pub const BARRIER_STAGE_ENV: &str = "PAIGASUS_TEST_BARRIER_STAGE";
/// The file that the barrier creates when the run reaches its stage.
pub const BARRIER_FILE_ENV: &str = "PAIGASUS_TEST_BARRIER_FILE";
/// The model name that [`register`] uses.
pub const MODEL_NAME: &str = "scripted";

/// Read the turns of a script file.
///
/// # Errors
///
/// The file cannot be read, or it is not a JSON array of turns.
pub fn read_script(path: &Path) -> anyhow::Result<Vec<Vec<ScriptedCall>>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read the test model script {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse the test model script {}", path.display()))
}

/// Register the `scripted` model from the script file that [`SCRIPT_ENV`] names. When the
/// variable is not set, it registers nothing.
///
/// # Errors
///
/// The variable is set, and [`read_script`] fails.
pub fn register(models: &mut ModelRegistry) -> anyhow::Result<()> {
    let Some(path) = std::env::var_os(SCRIPT_ENV) else {
        return Ok(());
    };
    let turns = read_script(Path::new(&path))?;
    models.register(MODEL_NAME, SharedModel::new(ScriptedModel::new(turns)));
    Ok(())
}

/// The stage that a [`BARRIER_STAGE_ENV`] value names, or `None` for an unknown value.
#[must_use]
pub fn parse_stage(value: &str) -> Option<FaultStage> {
    match value {
        "after_run" => Some(FaultStage::AfterRun),
        "before_commit" => Some(FaultStage::BeforeCommit),
        "after_commit" => Some(FaultStage::AfterCommit),
        _ => None,
    }
}

/// A fault point that, at its stage, creates its file and then never returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileBarrier {
    stage: FaultStage,
    path: PathBuf,
}

impl FileBarrier {
    /// A barrier at the stage that `stage` names, or `None` when the name is unknown.
    #[must_use]
    pub fn new(stage: &str, path: PathBuf) -> Option<Self> {
        parse_stage(stage).map(|stage| Self { stage, path })
    }

    /// The barrier of [`BARRIER_STAGE_ENV`] and [`BARRIER_FILE_ENV`]. See [`Self::from_vars`].
    #[must_use]
    pub fn from_env() -> Option<Self> {
        Self::from_vars(std::env::var(BARRIER_STAGE_ENV).ok(), std::env::var_os(BARRIER_FILE_ENV))
    }

    /// The barrier of a stage value and a file value. `None` when a value is absent or the stage
    /// is unknown. When exactly one value is set, or the stage is unknown, it logs a warning: a
    /// typo in one variable name must not turn the barrier off in silence.
    #[must_use]
    pub fn from_vars(stage: Option<String>, path: Option<OsString>) -> Option<Self> {
        let (stage, path) = match (stage, path) {
            (Some(stage), Some(path)) => (stage, path),
            (None, None) => return None,
            (Some(_), None) => {
                tracing::warn!(set = BARRIER_STAGE_ENV, missing = BARRIER_FILE_ENV, "only one test barrier variable is set; the barrier is off");
                return None;
            }
            (None, Some(_)) => {
                tracing::warn!(set = BARRIER_FILE_ENV, missing = BARRIER_STAGE_ENV, "only one test barrier variable is set; the barrier is off");
                return None;
            }
        };
        let barrier = Self::new(&stage, PathBuf::from(path));
        if barrier.is_none() {
            tracing::warn!(%stage, "the test barrier stage is unknown; the barrier is off");
        }
        barrier
    }
}

#[async_trait]
impl FaultPoint for FileBarrier {
    async fn reach(&self, stage: FaultStage) {
        if stage != self.stage {
            return;
        }
        if let Err(error) = std::fs::File::create(&self.path) {
            tracing::error!(%error, path = %self.path.display(), "the test barrier cannot create its file");
        }
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("{name}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn the_script_file_parses_into_turns() {
        let path = temp_path("test-model-script");
        std::fs::write(&path, r#"[[["c1", "memory", {"command": "view", "path": "/"}]]]"#).expect("write the script");
        let turns = read_script(&path).expect("parse");
        let _ = std::fs::remove_file(&path);
        assert_eq!(turns, vec![vec![("c1".to_owned(), "memory".to_owned(), json!({"command": "view", "path": "/"}))]]);
    }

    #[test]
    fn a_script_that_is_not_turns_is_an_error() {
        let path = temp_path("test-model-script");
        std::fs::write(&path, r#"{"turns": 1}"#).expect("write the script");
        let result = read_script(&path);
        let _ = std::fs::remove_file(&path);
        assert!(result.is_err());
    }

    #[test]
    fn an_unknown_barrier_stage_is_none() {
        assert_eq!(FileBarrier::new("later", PathBuf::from("/x")), None);
        assert_eq!(parse_stage("after_run"), Some(FaultStage::AfterRun));
        assert_eq!(parse_stage("before_commit"), Some(FaultStage::BeforeCommit));
        assert_eq!(parse_stage("after_commit"), Some(FaultStage::AfterCommit));
    }

    /// Counts the WARN events that reach it.
    #[derive(Default)]
    struct WarnCounter(std::sync::atomic::AtomicUsize);

    impl tracing::Subscriber for WarnCounter {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            if *event.metadata().level() == tracing::Level::WARN {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
        fn enter(&self, _span: &tracing::span::Id) {}
        fn exit(&self, _span: &tracing::span::Id) {}
    }

    /// The barrier of `stage` and `path`, and the number of warnings that the call logged.
    fn from_vars_warnings(stage: Option<&str>, path: Option<&str>) -> (Option<FileBarrier>, usize) {
        let counter = std::sync::Arc::new(WarnCounter::default());
        let barrier = tracing::subscriber::with_default(counter.clone(), || FileBarrier::from_vars(stage.map(str::to_owned), path.map(std::ffi::OsString::from)));
        (barrier, counter.0.load(std::sync::atomic::Ordering::SeqCst))
    }

    #[test]
    fn exactly_one_barrier_variable_is_a_warning() {
        // A typo in one name must not turn the barrier off in silence.
        assert_eq!(from_vars_warnings(Some("after_run"), None), (None, 1), "a stage with no file");
        assert_eq!(from_vars_warnings(None, Some("/x")), (None, 1), "a file with no stage");
        assert_eq!(from_vars_warnings(Some("later"), Some("/x")), (None, 1), "an unknown stage");
        assert_eq!(from_vars_warnings(None, None), (None, 0), "no variable is the normal case");
        assert_eq!(
            from_vars_warnings(Some("after_run"), Some("/x")),
            (FileBarrier::new("after_run", PathBuf::from("/x")), 0),
            "both variables give a barrier"
        );
    }

    #[tokio::test]
    async fn the_barrier_creates_its_file_at_its_stage_and_waits() {
        let path = temp_path("test-barrier");
        let barrier = FileBarrier::new("before_commit", path.clone()).expect("a known stage");
        // Another stage passes at once and creates no file.
        tokio::time::timeout(Duration::from_secs(1), barrier.reach(FaultStage::AfterRun)).await.expect("another stage returns");
        assert!(!path.exists());
        // Its own stage creates the file and does not return.
        let waited = tokio::time::timeout(Duration::from_millis(200), barrier.reach(FaultStage::BeforeCommit)).await;
        let created = path.exists();
        let _ = std::fs::remove_file(&path);
        assert!(waited.is_err(), "the barrier must not return at its stage");
        assert!(created, "the barrier must create its file at its stage");
    }
}
