// SPDX-License-Identifier: Apache-2.0
//! The public types of the tool policy decision. Every public type is `#[non_exhaustive]`, so a
//! caller makes a value with its `new` constructor.

use std::collections::BTreeSet;

use uuid::Uuid;

use crate::CedarUid;

/// The finding that two tool names have the same normal form in one request.
pub const NAME_COLLISION: &str = "name_collision";
/// The finding that the arguments of a call are not one strict JSON object.
pub const UNPARSED_ARGUMENTS: &str = "unparsed_arguments";
/// The finding that the calls of one history belong to more than one session.
pub const MIXED_SESSIONS: &str = "mixed_sessions";
/// The finding that the history of a request contains a call that the gateway did not see.
pub const FORGED_HISTORY: &str = "forged_history";
/// The finding that the history store cut the events of a history at a cap or at the retention limit.
pub const HISTORY_TRUNCATED: &str = "history_truncated";

/// The arguments of a tool call, as the model sent them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolInput {
    /// The raw JSON text. The kernel parses it and reads each number from its original text.
    Json(String),
    /// Free text, for the tool kinds `custom`, `apply_patch` and `other`.
    Text(String),
}

/// Whether the name of a call is among the tool definitions of the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Declared {
    /// The name is among the definitions.
    Yes,
    /// The definitions are complete and the name is not among them.
    No,
    /// The definitions are not complete, so the kernel cannot tell.
    Unknown,
}

impl Declared {
    /// The value in `context.call.declared`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Yes => "yes",
            Self::No => "no",
            Self::Unknown => "unknown",
        }
    }
}

/// One tool call to decide.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolInvocation {
    /// The raw name as the model sent it (`namespace.name` if it has a namespace).
    pub name: String,
    /// The arguments.
    pub input: ToolInput,
    /// Whether the name is among the request definitions.
    pub declared: Declared,
}

impl ToolInvocation {
    /// Make an invocation.
    #[must_use]
    pub fn new(name: impl Into<String>, input: ToolInput, declared: Declared) -> Self {
        Self { name: name.into(), input, declared }
    }
}

/// The facts of the request that are the same for every call in it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RequestInfo {
    /// The scope PRN of the request.
    pub scope: String,
    /// `chat_completions`, `anthropic_messages` or `responses`.
    pub ingress: String,
    /// The model of the request.
    pub model: String,
}

impl RequestInfo {
    /// Make the request facts.
    #[must_use]
    pub fn new(scope: impl Into<String>, ingress: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            scope: scope.into(),
            ingress: ingress.into(),
            model: model.into(),
        }
    }
}

/// The session facts. The gateway has no session yet, so it passes the placeholder (D14).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SessionInfo {
    /// The session id.
    pub id: String,
    /// The turn number in the session.
    pub turn: i64,
}

impl SessionInfo {
    /// Make the session facts.
    #[must_use]
    pub fn new(id: impl Into<String>, turn: i64) -> Self {
        Self { id: id.into(), turn }
    }

    /// The placeholder of D14: an empty id and turn 0.
    #[must_use]
    pub fn placeholder() -> Self {
        Self { id: String::new(), turn: 0 }
    }
}

/// The facts for one decision.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Facts {
    /// The principal, from `to_cedar_uid` of the principal PRN.
    pub principal: CedarUid,
    /// The tenant (D7, D20).
    pub org: Uuid,
    /// The request facts.
    pub request: RequestInfo,
    /// The session facts.
    pub session: SessionInfo,
}

impl Facts {
    /// Make the facts.
    #[must_use]
    pub fn new(principal: CedarUid, org: Uuid, request: RequestInfo, session: SessionInfo) -> Self {
        Self { principal, org, request, session }
    }
}

/// The set of finding kinds that the gateway saw about a request or a call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Findings(BTreeSet<String>);

impl Findings {
    /// Make an empty set.
    #[must_use]
    pub fn new() -> Self {
        Self(BTreeSet::new())
    }

    /// Add a finding kind.
    pub fn insert(&mut self, kind: &str) {
        self.0.insert(kind.to_owned());
    }

    /// Whether the set holds the kind.
    #[must_use]
    pub fn contains(&self, kind: &str) -> bool {
        self.0.contains(kind)
    }

    /// The kinds, in sorted order.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_strings() {
        assert_eq!(Declared::Yes.as_str(), "yes");
        assert_eq!(Declared::No.as_str(), "no");
        assert_eq!(Declared::Unknown.as_str(), "unknown");
    }

    #[test]
    fn session_placeholder_is_empty_turn_zero() {
        let s = SessionInfo::placeholder();
        assert_eq!((s.id.as_str(), s.turn), ("", 0));
    }

    #[test]
    fn findings_keep_sorted_unique_kinds() {
        let mut f = Findings::new();
        assert!(!f.contains(NAME_COLLISION));
        f.insert(UNPARSED_ARGUMENTS);
        f.insert(NAME_COLLISION);
        f.insert(NAME_COLLISION);
        assert!(f.contains(NAME_COLLISION) && f.contains(UNPARSED_ARGUMENTS));
        assert_eq!(f.iter().collect::<Vec<_>>(), [NAME_COLLISION, UNPARSED_ARGUMENTS]);
    }

    #[test]
    fn constructors_fill_every_field() {
        let uid = CedarUid {
            entity_type: "Pgs::Iam::Principal".into(),
            entity_id: "x".into(),
        };
        let org = Uuid::nil();
        let f = Facts::new(uid.clone(), org, RequestInfo::new("s", "responses", "m"), SessionInfo::new("sid", 3));
        assert_eq!((f.principal, f.org), (uid, org));
        assert_eq!((f.request.scope.as_str(), f.request.ingress.as_str(), f.request.model.as_str()), ("s", "responses", "m"));
        assert_eq!((f.session.id.as_str(), f.session.turn), ("sid", 3));
        let c = ToolInvocation::new("n", ToolInput::Text("t".into()), Declared::No);
        assert_eq!((c.name.as_str(), c.input, c.declared), ("n", ToolInput::Text("t".into()), Declared::No));
    }
}
