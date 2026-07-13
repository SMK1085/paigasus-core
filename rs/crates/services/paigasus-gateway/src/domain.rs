// SPDX-License-Identifier: Apache-2.0

//! The authenticated caller identity a request carries after the G5 auth middleware
//! validates its bearer credential; consumed by the chat handler (G7) to authorize the
//! request and to log request metadata (never the prompt/response body or any other PII).

/// The caller identity resolved from a request's bearer credential (an IAM API key or
/// service-account token — G5 populates this via the IAM introspect call). Carried through
/// the request extensions so downstream handlers (G7) never need to re-authenticate.
#[derive(Debug, Clone)]
pub struct CallerContext {
    /// The authenticated principal's PRN (Paigasus Resource Name), e.g. a service account.
    pub principal_prn: String,
    /// The scope PRN the caller's credential was issued under (org/team/project), used to
    /// authorize the request against the target resource.
    pub scope_prn: String,
    /// The credential's own identifier (the API key's `key_id`, not the secret itself) —
    /// safe to log; identifies which key authenticated the request without leaking it.
    pub key_id: String,
}
