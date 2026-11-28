// SPDX-License-Identifier: Apache-2.0

//! The `AgentRunner` port (spec §6.5, §6.7). It lives in the service and not in the core, because
//! `RunSpec` holds a tokio `Mutex` and a cancellation token. Every other type in it is a core type:
//! the Helikon adapter converts from the Helikon types.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use paigasus_agent_host_core::agent::AgentDefinition;
use paigasus_agent_host_core::ids::ThreadId;
use paigasus_agent_host_core::memory::MemoryWorkspace;
use paigasus_agent_host_core::run::{RunUsage, SessionRow};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// The user turn of a run (spec §6.4): one user message with up to two text parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserTurn {
    /// The run-start memory text. `None` when no memory file exists: the part is then absent.
    pub memory: Option<String>,
    /// The rendered events of the run.
    pub events: String,
}

/// The limits of one run (spec §6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunLimits {
    pub max_turns: u32,
    pub timeout: Duration,
}

/// Everything that one run needs.
pub struct RunSpec {
    pub agent: AgentDefinition,
    pub thread: ThreadId,
    /// A fresh thread has no committed session events, so the session read is skipped.
    pub fresh_thread: bool,
    pub turn: UserTurn,
    /// The workspace of the run. The `memory` tool and the run commit share it.
    pub workspace: Arc<Mutex<MemoryWorkspace>>,
    /// The run token (spec §6.1). Cancelling it stops the run. The end of a cancelled run is not
    /// always `RunEnd::Cancelled`: runtime-tokio 0.1.26 can also end it as `Transient`,
    /// `Completed` or `Deterministic`.
    pub cancel: CancellationToken,
    pub limits: RunLimits,
}

/// How a run ended, in the classes of spec §6.7. The worker checks the lease and the shutdown
/// token after EVERY end, because only it knows them, and a cancel can give any end: a lost
/// lease makes the run Discarded (no write), then a cancelled shutdown token makes it Shutdown.
/// Only when neither holds does the end decide the class, and `Cancelled` is then Transient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEnd {
    Completed {
        output: String,
        usage: RunUsage,
    },
    Stopped {
        output: String,
        reason: String,
        usage: RunUsage,
    },
    Deterministic {
        error: String,
        close_thread: bool,
    },
    Transient {
        error: String,
        retry_after: Option<Duration>,
    },
    /// The fault is in the host or the gateway link, not in the event. The worker backs off and
    /// gives the attempt back.
    HostFault {
        error: String,
        retry_after: Option<Duration>,
    },
    Cancelled,
}

/// The end of a run and the session events of the run, in their stored form. The worker writes
/// the session only on a full commit.
#[derive(Debug, Clone, PartialEq)]
pub struct RunOutput {
    pub end: RunEnd,
    pub session: Vec<SessionRow>,
}

/// Run one agent turn.
#[async_trait]
pub trait AgentRunner: Send + Sync {
    async fn run(&self, spec: RunSpec) -> RunOutput;
}
