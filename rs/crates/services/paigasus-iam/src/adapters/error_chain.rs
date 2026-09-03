// SPDX-License-Identifier: Apache-2.0

//! Rendering an error and its full `source()` chain as one line.
//!
//! Several of this crate's error types carry a static `Display` and put the real cause in
//! `source()` — `AuthnError::Backend`'s is the literal `"backend error"`, and reqwest's
//! client-build error is the literal `"builder error"`. For those, `to_string()` alone tells an
//! operator nothing, so anything that renders one into a message, a log line or a stored column
//! walks the chain first.
//!
//! Lived in `adapters/events/relay.rs` until SMA-570 needed it in `adapters/oidc/jwks.rs` too.

/// Renders `err` and its full `source()` chain as `"outer: middle: inner"`.
pub(crate) fn describe_error(err: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(e) = source {
        parts.push(e.to_string());
        source = e.source();
    }
    parts.join(": ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use paigasus_iam_core::PublishError;

    /// An error must carry its whole `source()` chain into the rendered string —
    /// some error types' own `Display` is static and renders nothing about the actual cause.
    /// For those, `describe_error` is what makes the error informative.
    #[test]
    fn describe_error_walks_the_full_source_chain_without_duplicating_levels() {
        #[derive(Debug, thiserror::Error)]
        #[error("transport closed")]
        struct Inner;

        #[derive(Debug, thiserror::Error)]
        #[error("publish failed")]
        struct Outer(#[source] Inner);

        let err = PublishError::from(Box::new(Outer(Inner)) as Box<dyn std::error::Error + Send + Sync>);
        assert_eq!(describe_error(&err), "backend error: publish failed: transport closed");
    }

    #[test]
    fn describe_error_of_a_sourceless_error_is_just_its_display() {
        #[derive(Debug, thiserror::Error)]
        #[error("nope")]
        struct Bare;
        assert_eq!(describe_error(&Bare), "nope");
    }
}
