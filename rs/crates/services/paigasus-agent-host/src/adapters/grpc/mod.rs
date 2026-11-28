// SPDX-License-Identifier: Apache-2.0

//! The gRPC `AgentService` of the agent host (SMA-852 spec §6.5, §6.6). It serves the same rules
//! as the REST routes: both transports call `AgentAdmin`.

pub mod agent_service;
pub mod status;

pub use self::agent_service::AgentGrpcSlot;
pub use self::status::to_status;
