// SPDX-License-Identifier: Apache-2.0
//! The decider port (spec D18). SMA-752 adds the Cedar decider behind the same trait.
//!
//! The arguments are owned (`Arc`s and a `Vec`), and the future is `'static`, so the gateway can
//! keep a pending decision inside its relay state without borrowing the interceptor.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::interceptor::DeciderError;
use crate::policy::PolicySnapshot;
use crate::turn::ToolTurn;
use crate::types::{Decision, ToolCall};

/// Who is asking, captured in the handler. Inside the stream `current_ids()` is `None`
/// (SMA-504 § 4.3), so the request id comes in here.
///
/// No `PartialEq`: the policy snapshot holds a Cedar policy set, and nothing compares a context
/// (preflight F7).
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct DecisionContext {
    pub principal: String,
    pub scope: String,
    pub request_id: Option<String>,
    /// The request's policy snapshot (D30). `None` when the tool policy is off.
    pub policy: Option<Arc<PolicySnapshot>>,
    /// The id of the API key of the request, when it has one (SMA-755). Empty by default.
    pub api_key_id: Option<String>,
    /// The ingress label of the request (SMA-755). Empty by default.
    pub ingress: String,
    /// The model of the request (SMA-755). Empty by default.
    pub model: String,
}

impl DecisionContext {
    #[must_use]
    pub fn new(principal: impl Into<String>, scope: impl Into<String>, request_id: Option<String>) -> Self {
        DecisionContext {
            principal: principal.into(),
            scope: scope.into(),
            request_id,
            policy: None,
            api_key_id: None,
            ingress: String::new(),
            model: String::new(),
        }
    }

    /// Attach the request facts that the decision log records (SMA-755).
    #[must_use]
    pub fn with_request(mut self, api_key_id: Option<String>, ingress: impl Into<String>, model: impl Into<String>) -> Self {
        self.api_key_id = api_key_id;
        self.ingress = ingress.into();
        self.model = model.into();
        self
    }

    /// Attach the request's policy snapshot (D30).
    #[must_use]
    pub fn with_policy(mut self, snapshot: Arc<PolicySnapshot>) -> Self {
        self.policy = Some(snapshot);
        self
    }
}

pub type DecideFuture = Pin<Box<dyn Future<Output = Result<Vec<Decision>, DeciderError>> + Send + 'static>>;

/// Decides one completed hold. MUST return exactly one decision per call, in order: the
/// interceptor treats any other length as a decider error (D18).
pub trait ToolCallDecider: Send + Sync + 'static {
    fn decide(&self, ctx: Arc<DecisionContext>, turn: Arc<ToolTurn>, calls: Vec<ToolCall>) -> DecideFuture;
}

/// The only decider in SMA-749.
#[derive(Debug, Clone, Copy, Default)]
pub struct AllowAll;

impl ToolCallDecider for AllowAll {
    fn decide(&self, _ctx: Arc<DecisionContext>, _turn: Arc<ToolTurn>, calls: Vec<ToolCall>) -> DecideFuture {
        let n = calls.len();
        Box::pin(std::future::ready(Ok(vec![Decision::Allow; n])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ToolCall, ToolCallKind};

    fn call() -> ToolCall {
        ToolCall {
            choice: 0,
            position: 0,
            id: None,
            name: "x".into(),
            namespace: None,
            kind: ToolCallKind::Function,
            arguments: String::new(),
        }
    }

    fn ready(mut fut: DecideFuture) -> Result<Vec<Decision>, DeciderError> {
        // The future is ready at once; poll it without a runtime.
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match fut.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(r) => r,
            std::task::Poll::Pending => panic!("pending"),
        }
    }

    #[test]
    fn allow_all_allows_every_call() {
        let fut = AllowAll.decide(Arc::new(DecisionContext::new("p", "s", None)), Arc::new(ToolTurn::Unavailable("test")), vec![call(), call()]);
        assert_eq!(ready(fut), Ok(vec![Decision::Allow, Decision::Allow]));
    }

    #[test]
    fn a_new_context_has_no_policy_and_with_policy_sets_it() {
        use crate::policy::{OnUnavailable, PolicySet, PolicySnapshot, TenantMode};
        let ctx = DecisionContext::new("p", "s", None);
        assert!(ctx.policy.is_none());
        let snap = Arc::new(PolicySnapshot::new(PolicySet::Unavailable, TenantMode::Enforce, OnUnavailable::Closed));
        let ctx = ctx.with_policy(Arc::clone(&snap));
        assert!(ctx.policy.as_ref().is_some_and(|p| Arc::ptr_eq(p, &snap)));
    }

    // Compile-time proof that the trait is object safe (the gateway holds it as `Arc<dyn …>`).
    const _: fn(&dyn ToolCallDecider) = |_| {};

    #[test]
    fn the_decider_works_through_a_trait_object() {
        let d: Arc<dyn ToolCallDecider> = Arc::new(AllowAll);
        let fut = d.decide(Arc::new(DecisionContext::new("p", "s", Some("r".into()))), Arc::new(ToolTurn::Unavailable("test")), vec![call()]);
        assert_eq!(ready(fut), Ok(vec![Decision::Allow]));
    }
}
