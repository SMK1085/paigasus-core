// SPDX-License-Identifier: Apache-2.0
//! The shapes the SMA-747 specs share (spec § 4.1).
//!
//! Every type is `#[non_exhaustive]`: SMA-752 and SMA-754 add variants, and that must not be a
//! breaking change. These types move to `paigasus-kernel` with SMA-752, when a binding needs
//! them (spec D1).

/// Which wire shape a tool call came from.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolCallKind {
    /// OpenAI `tool_calls[]` of type `function`.
    Function,
    /// OpenAI `tool_calls[]` of type `custom`.
    Custom,
    /// OpenAI's legacy `function_call` field (the `functions` API).
    LegacyFunctionCall,
    /// An Anthropic `tool_use` content block.
    AnthropicToolUse,
    /// Any other tool-call shape. Held and decided, never passed unchecked (fail closed).
    Other,
}

impl ToolCallKind {
    /// A stable lower-snake label, for reports and tests.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ToolCallKind::Function => "function",
            ToolCallKind::Custom => "custom",
            ToolCallKind::LegacyFunctionCall => "legacy_function_call",
            ToolCallKind::AnthropicToolUse => "tool_use",
            ToolCallKind::Other => "other",
        }
    }
}

/// One complete tool call, as the decider sees it.
///
/// `arguments` holds (spec § 4.1): the concatenated `function.arguments` string (Function,
/// LegacyFunctionCall); `custom.input` (Custom); the concatenated `partial_json` of an Anthropic
/// stream, or the compact JSON of a non-stream `input` (AnthropicToolUse); and the raw JSON of the
/// whole entry, as a JSON array of its fragments (Other).
///
/// `Debug` prints `arguments`, which are model output and may carry secrets. Never log a
/// `ToolCall` with `{:?}`; log its sanitized name only (spec § 4.5).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    /// OpenAI choice index. Always 0 for Anthropic.
    pub choice: u32,
    /// The tool-call index (OpenAI) or the content-block index (Anthropic), as the upstream sent it.
    pub position: u32,
    /// The upstream's call id. `None` for a legacy `function_call`.
    pub id: Option<String>,
    pub name: String,
    pub kind: ToolCallKind,
    pub arguments: String,
}

/// The decider's answer for one call.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Block { code: BlockCode },
}

impl Decision {
    #[must_use]
    pub fn is_allow(self) -> bool {
        matches!(self, Decision::Allow)
    }
}

/// Why a call was blocked.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockCode {
    /// A policy refused it (SMA-752).
    Policy,
    /// A detector finding refused it (SMA-754).
    Detector,
    /// A hold exceeded `max_held_bytes` or `max_hold_secs` (D13, D14).
    HoldLimit,
    /// A record or the assembly was not in canonical form (D5–D9).
    Malformed,
    /// The stream ended before the call was complete (D10, D11).
    Truncated,
    /// The decider failed, timed out, or returned the wrong number of decisions (D18).
    DeciderError,
}

impl BlockCode {
    /// The kebab-case label used in the notice and in reports (§ 4.5).
    #[must_use]
    pub fn as_kebab(self) -> &'static str {
        match self {
            BlockCode::Policy => "policy",
            BlockCode::Detector => "detector",
            BlockCode::HoldLimit => "hold-limit",
            BlockCode::Malformed => "malformed",
            BlockCode::Truncated => "truncated",
            BlockCode::DeciderError => "decider-error",
        }
    }
}

/// How a decision was reached (§ 5.3).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    Decider,
    OnFailure,
}

impl Basis {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Basis::Decider => "decider",
            Basis::OnFailure => "on_failure",
        }
    }
}

/// One hold: an OpenAI choice, or an Anthropic content block (D10, D11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HoldKey {
    Choice(u32),
    Block(u32),
}

/// Which provider wire format a stream or body uses.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    OpenAiChat,
    AnthropicMessages,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_codes_have_stable_kebab_labels() {
        let all = [
            BlockCode::Policy,
            BlockCode::Detector,
            BlockCode::HoldLimit,
            BlockCode::Malformed,
            BlockCode::Truncated,
            BlockCode::DeciderError,
        ];
        let labels: Vec<&str> = all.iter().map(|c| c.as_kebab()).collect();
        assert_eq!(labels, ["policy", "detector", "hold-limit", "malformed", "truncated", "decider-error"]);
    }

    #[test]
    fn only_allow_is_allow() {
        assert!(Decision::Allow.is_allow());
        assert!(!Decision::Block { code: BlockCode::Policy }.is_allow());
    }

    #[test]
    fn kinds_and_bases_have_labels() {
        assert_eq!(ToolCallKind::LegacyFunctionCall.as_str(), "legacy_function_call");
        assert_eq!(ToolCallKind::AnthropicToolUse.as_str(), "tool_use");
        assert_eq!(Basis::OnFailure.as_str(), "on_failure");
    }

    #[test]
    fn hold_keys_order_choices_before_blocks() {
        let mut keys = vec![HoldKey::Block(0), HoldKey::Choice(1), HoldKey::Choice(0)];
        keys.sort();
        assert_eq!(keys, [HoldKey::Choice(0), HoldKey::Choice(1), HoldKey::Block(0)]);
    }
}
