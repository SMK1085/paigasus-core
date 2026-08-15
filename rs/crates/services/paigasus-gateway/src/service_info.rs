// SPDX-License-Identifier: Apache-2.0

//! The gateway's capability projection — the same shape as IAM's `service_info`, over
//! `GatewayConfig`. `enabled()` is a pure function, so AC 3's descriptor half is a unit test
//! with no `AppState` and no network.

use paigasus_proto::paigasus::common::v1::{Capability, ServiceInfo};

use crate::config::GatewayConfig;

/// The bare service slug, matching the prefix of this service's own capability keys.
pub const SERVICE: &str = "gateway";

/// This build's version — `env!` evaluated in THIS crate, so it is `paigasus-gateway`'s own
/// `Cargo.toml` version (AC 4). Reports `0.0.0` until release-plz is activated.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// `stream_enabled` -> `gateway.chat.stream`.
    pub chat_stream: bool,
}

impl Capabilities {
    #[must_use]
    pub fn from_config(cfg: &GatewayConfig) -> Self {
        Capabilities { chat_stream: cfg.stream_enabled }
    }

    #[must_use]
    pub fn enabled(&self) -> Vec<Capability> {
        if self.chat_stream { vec![Capability::GatewayChatStream] } else { Vec::new() }
    }

    #[must_use]
    pub fn descriptor(&self) -> ServiceInfo {
        paigasus_service_info::descriptor(SERVICE, VERSION, &self.enabled())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paigasus_proto::paigasus::common::v1::Capability;

    #[test]
    fn streaming_enabled_advertises_the_capability() {
        assert_eq!(Capabilities { chat_stream: true }.enabled(), vec![Capability::GatewayChatStream]);
    }

    /// AC 3 for the gateway: the flag off removes the key, and the descriptor still serializes
    /// the field as an empty array rather than dropping it.
    #[test]
    fn streaming_disabled_advertises_nothing() {
        assert!(Capabilities { chat_stream: false }.enabled().is_empty());
        let info = Capabilities { chat_stream: false }.descriptor();
        assert!(info.capabilities.is_empty());
        assert_eq!(info.service, "gateway");
    }

    #[test]
    fn the_descriptor_names_this_crates_build_version() {
        let info = Capabilities { chat_stream: true }.descriptor();
        // `env!` expanded HERE, not read back from the module's `VERSION` const — see the
        // identical assertion in paigasus-iam's `service_info` tests for why that distinction
        // is what gives this test content.
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        let core = info.version.split(['-', '+']).next().expect("a version always has a core");
        let parts: Vec<&str> = core.split('.').collect();
        assert_eq!(parts.len(), 3, "version core must be major.minor.patch: {}", info.version);
        assert!(
            parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())),
            "version core must be numeric: {}",
            info.version
        );
    }

    #[test]
    fn every_advertised_string_is_a_registered_capability_key() {
        for key in &(Capabilities { chat_stream: true }).descriptor().capabilities {
            assert!(Capability::from_wire_key(key).is_some(), "{key} is not in the registry");
        }
    }
}
