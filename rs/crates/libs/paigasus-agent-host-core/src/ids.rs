// SPDX-License-Identifier: Apache-2.0

//! Typed IDs of the agent host. PRN newtypes, UUID newtypes and string newtypes.

use paigasus_kernel::resource_name::Prn;
use uuid::Uuid;

use crate::error::DomainError;

/// The service segment of every agent-host PRN.
const SERVICE: &str = "agents";

/// The maximum length of a [`CallId`], in characters.
pub const CALL_ID_MAX_CHARS: usize = 256;
/// The maximum length of a [`DeferralId`], in characters.
pub const DEFERRAL_ID_MAX_CHARS: usize = 64;

/// Define an ID that wraps a `prn:pgs:agents::<org>:<type>/<uuid>` PRN.
macro_rules! org_prn_id {
    ($(#[$meta:meta])* $name:ident, $rtype:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(Prn);

        impl $name {
            /// Build the ID from an organization UUID and a resource UUID.
            #[must_use]
            pub fn new(org: Uuid, id: Uuid) -> Self {
                $name(Prn::build(SERVICE, "", Some(org), $rtype, id).expect("static prn parts are valid"))
            }

            /// Parse a canonical PRN. The service, the type, the region and the organization
            /// must match this ID kind.
            pub fn parse(raw: &str) -> Result<Self, DomainError> {
                let err = || DomainError::InvalidId { kind: $rtype };
                let prn = Prn::parse(raw).map_err(|_| err())?;
                if prn.service() != SERVICE || prn.resource_type() != $rtype || !prn.region().is_empty() || prn.org().is_none() {
                    return Err(err());
                }
                Ok($name(prn))
            }

            #[must_use]
            pub fn as_prn(&self) -> &Prn {
                &self.0
            }

            #[must_use]
            pub fn uuid(&self) -> Uuid {
                self.0.resource_id()
            }

            #[must_use]
            pub fn org(&self) -> Uuid {
                self.0.org().expect("the constructor guarantees an organization")
            }
        }
    };
}

org_prn_id!(
    /// The identity of an agent.
    AgentId,
    "agent"
);
org_prn_id!(
    /// The identity of an agent inbox.
    InboxId,
    "inbox"
);
org_prn_id!(
    /// The identity of an agent run.
    RunId,
    "run"
);

/// The identity of a principal: `prn:pgs:iam:::principal/<uuid>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PrincipalId(Prn);

impl PrincipalId {
    #[must_use]
    pub fn from_uuid(uuid: Uuid) -> Self {
        PrincipalId(Prn::build("iam", "", None, "principal", uuid).expect("static principal prn parts are valid"))
    }

    /// Parse a canonical principal PRN. It has no region and no organization.
    pub fn parse(raw: &str) -> Result<Self, DomainError> {
        let err = || DomainError::InvalidId { kind: "principal" };
        let prn = Prn::parse(raw).map_err(|_| err())?;
        if prn.service() != "iam" || prn.resource_type() != "principal" || !prn.region().is_empty() || prn.org().is_some() {
            return Err(err());
        }
        Ok(PrincipalId(prn))
    }

    #[must_use]
    pub fn as_prn(&self) -> &Prn {
        &self.0
    }

    #[must_use]
    pub fn uuid(&self) -> Uuid {
        self.0.resource_id()
    }
}

/// Define an ID that wraps a bare UUID.
macro_rules! uuid_id {
    ($(#[$meta:meta])* $name:ident, $kind:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(Uuid);

        impl $name {
            #[must_use]
            pub fn from_uuid(uuid: Uuid) -> Self {
                $name(uuid)
            }

            #[must_use]
            pub fn uuid(&self) -> Uuid {
                self.0
            }

            /// Parse a hyphenated UUID.
            pub fn parse(raw: &str) -> Result<Self, DomainError> {
                if raw.len() != 36 {
                    return Err(DomainError::InvalidId { kind: $kind });
                }
                Uuid::try_parse(raw).map($name).map_err(|_| DomainError::InvalidId { kind: $kind })
            }
        }
    };
}

uuid_id!(
    /// The identity of a conversation thread.
    ThreadId,
    "thread"
);
uuid_id!(
    /// The identity of an event.
    EventId,
    "event"
);
uuid_id!(
    /// The identity of a timer.
    TimerId,
    "timer"
);
uuid_id!(
    /// The identity of an outbox message.
    OutboxMessageId,
    "outbox_message"
);

/// Define an ID that wraps a bounded string.
macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident, $kind:literal, $max:expr) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(String);

        impl $name {
            /// The string is not empty, has at most the maximum number of characters, does not
            /// start or end with whitespace, and holds no NUL (U+0000).
            pub fn parse(raw: &str) -> Result<Self, DomainError> {
                let chars = raw.chars().count();
                if chars == 0 || chars > $max || raw.starts_with(char::is_whitespace) || raw.ends_with(char::is_whitespace) || raw.contains('\0') {
                    return Err(DomainError::InvalidId { kind: $kind });
                }
                Ok($name(raw.to_string()))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

string_id!(
    /// The provider-given identity of a tool call.
    CallId,
    "call",
    CALL_ID_MAX_CHARS
);
string_id!(
    /// The identity of a deferral.
    DeferralId,
    "deferral",
    DEFERRAL_ID_MAX_CHARS
);

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn agent_id_round_trips_through_its_prn() {
        let id = AgentId::new(u(1), u(2));
        assert_eq!(AgentId::parse(&id.as_prn().canonical()), Ok(id.clone()));
        assert_eq!(id.org(), u(1));
        assert_eq!(id.uuid(), u(2));
    }

    #[test]
    fn agent_id_prn_has_the_agents_service_and_no_region() {
        let id = AgentId::new(u(1), u(2));
        assert_eq!(id.as_prn().canonical(), format!("prn:pgs:agents::{}:agent/{}", u(1).as_hyphenated(), u(2).as_hyphenated()));
    }

    #[test]
    fn id_parse_rejects_another_service() {
        let raw = format!("prn:pgs:iam::{}:agent/{}", u(1).as_hyphenated(), u(2).as_hyphenated());
        assert!(AgentId::parse(&raw).is_err());
    }

    #[test]
    fn id_parse_rejects_another_type() {
        let raw = AgentId::new(u(1), u(2)).as_prn().canonical();
        assert!(InboxId::parse(&raw).is_err());
    }

    #[test]
    fn id_parse_rejects_a_region() {
        let raw = format!("prn:pgs:agents:eu-1:{}:run/{}", u(1).as_hyphenated(), u(2).as_hyphenated());
        assert!(RunId::parse(&raw).is_err());
    }

    #[test]
    fn id_parse_rejects_an_empty_org() {
        let raw = format!("prn:pgs:agents:::inbox/{}", u(2).as_hyphenated());
        assert!(InboxId::parse(&raw).is_err());
    }

    #[test]
    fn principal_id_rejects_an_org_or_a_region() {
        let (o, i) = (u(1).to_string(), u(2).to_string());
        assert!(PrincipalId::parse(&format!("prn:pgs:iam::{o}:principal/{i}")).is_err());
        assert!(PrincipalId::parse(&format!("prn:pgs:iam:eu-1::principal/{i}")).is_err());
    }

    #[test]
    fn principal_id_rejects_another_type() {
        assert!(PrincipalId::parse(&format!("prn:pgs:iam:::user/{}", u(2).as_hyphenated())).is_err());
    }

    #[test]
    fn principal_id_spellings_never_name_another_principal() {
        let id = PrincipalId::from_uuid(u(0xabcdef));
        assert_eq!(PrincipalId::parse(&id.as_prn().canonical()), Ok(id.clone()));
        let uuid = id.uuid().to_string();
        let upper = id.as_prn().canonical().replace(&uuid, &uuid.to_uppercase());
        if let Ok(parsed) = PrincipalId::parse(&upper) {
            assert_eq!(parsed, id);
        }
    }

    #[test]
    fn thread_id_parse_rejects_a_non_uuid() {
        assert!(ThreadId::parse("not-a-uuid").is_err());
        assert_eq!(ThreadId::parse(&u(7).as_hyphenated().to_string()), Ok(ThreadId::from_uuid(u(7))));
    }

    #[test]
    fn call_id_rejects_empty_nul_and_outer_whitespace() {
        for bad in ["", "a\0b", " a", "a "] {
            assert!(CallId::parse(bad).is_err(), "{bad:?}");
        }
        assert_eq!(CallId::parse("call_1").expect("valid").as_str(), "call_1");
    }

    #[test]
    fn call_id_limit_counts_characters_not_bytes() {
        assert!(CallId::parse(&"é".repeat(CALL_ID_MAX_CHARS)).is_ok());
        assert!(CallId::parse(&"é".repeat(CALL_ID_MAX_CHARS + 1)).is_err());
    }

    #[test]
    fn deferral_id_limit_is_64_characters() {
        assert!(DeferralId::parse(&"a".repeat(DEFERRAL_ID_MAX_CHARS)).is_ok());
        assert!(DeferralId::parse(&"a".repeat(DEFERRAL_ID_MAX_CHARS + 1)).is_err());
        assert!(DeferralId::parse("x").is_ok());
    }
}
