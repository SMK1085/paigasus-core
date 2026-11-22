// SPDX-License-Identifier: Apache-2.0

//! The fixed error classes of the S3 memory store (spec §6.4, AG5 and AG8).
//!
//! An `object_store` error can hold the object key, the endpoint URL or a response body. No
//! such text leaves this module. Each error maps to one fixed class text, and the adapter
//! builds each `Store(String)` from `store_text`.

use object_store::Error;
use object_store::client::{HttpError, HttpErrorKind};

/// The text that comes before the status code in the `Display` text of a failed request.
const STATUS_MARKER: &str = "non-2xx status code: ";

/// The class text of a request that ran out of time.
pub const TIMEOUT_CLASS: &str = "timeout";

/// The class of an error, as a fixed text. The text never holds a path or a URL.
pub fn class(e: &Error) -> String {
    match e {
        Error::NotFound { .. } => "not_found".to_owned(),
        Error::Precondition { .. } => "precondition".to_owned(),
        Error::AlreadyExists { .. } => "already_exists".to_owned(),
        Error::PermissionDenied { .. } => "permission_denied".to_owned(),
        Error::Unauthenticated { .. } => "unauthenticated".to_owned(),
        Error::NotImplemented { .. } => "not_implemented".to_owned(),
        Error::InvalidPath { .. } => "unparsable_key".to_owned(),
        Error::Generic { source, .. } => generic_class(&**source),
        _ => "other".to_owned(),
    }
}

/// Find the class of a `Generic` error from its chain of sources.
fn generic_class(source: &(dyn std::error::Error + 'static)) -> String {
    let mut status = None;
    let mut link = Some(source);
    while let Some(err) = link {
        if let Some(http) = err.downcast_ref::<HttpError>() {
            match http.kind() {
                HttpErrorKind::Timeout => return TIMEOUT_CLASS.to_owned(),
                // `Unknown` is an error that reqwest cannot sort. The server can still get the request.
                HttpErrorKind::Connect | HttpErrorKind::Request | HttpErrorKind::Interrupted | HttpErrorKind::Decode | HttpErrorKind::Unknown => {
                    return "transport".to_owned();
                }
                _ => {}
            }
        }
        if status.is_none() {
            status = status_code(&err.to_string());
        }
        link = err.source();
    }
    match status {
        Some(code) => format!("status={code}"),
        None => "other".to_owned(),
    }
}

/// Take the three digits after the status marker. The rest of the text is dropped.
fn status_code(text: &str) -> Option<String> {
    let at = text.find(STATUS_MARKER)? + STATUS_MARKER.len();
    let digits: String = text[at..].chars().take(3).collect();
    (digits.len() == 3 && digits.bytes().all(|b| b.is_ascii_digit())).then_some(digits)
}

/// True when the store refused the write because of the condition: the object exists, or
/// the precondition failed (spec §6.4).
pub fn is_refusal(e: &Error) -> bool {
    matches!(e, Error::AlreadyExists { .. } | Error::Precondition { .. })
}

/// True when the class is `timeout`, `transport` or a 5xx status other than 501. The server can
/// still run the request after the client gave up, so a check read cannot prove that the request
/// failed. A 5xx does not prove that the write stopped. A 501 does: the store does not do the
/// operation. A 4xx refusal is not uncertain.
pub fn is_uncertain(e: &Error) -> bool {
    let class = class(e);
    match class.as_str() {
        TIMEOUT_CLASS | "transport" => true,
        "status=501" => false,
        other => other.strip_prefix("status=5").is_some(),
    }
}

/// The text of a `Store(String)`: the fixed sentence and the class in brackets.
pub fn store_text(sentence: &'static str, e: &Error) -> String {
    format!("{sentence} ({})", class(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;

    const SECRET: &str = "secret-name";

    #[derive(Debug)]
    struct Leaky(String);

    impl fmt::Display for Leaky {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.0)
        }
    }

    impl std::error::Error for Leaky {}

    fn leaky() -> Box<dyn std::error::Error + Send + Sync> {
        Box::new(Leaky(format!("https://store.example/{SECRET}?X-Amz-Credential={SECRET}")))
    }

    fn path() -> String {
        format!("mem/{SECRET}/x")
    }

    fn http(kind: HttpErrorKind) -> Error {
        Error::Generic {
            store: "S3",
            source: Box::new(HttpError::new(kind, Leaky(format!("https://store.example/{SECRET}")))),
        }
    }

    fn all_kinds() -> Vec<(Error, &'static str)> {
        vec![
            (Error::NotFound { path: path(), source: leaky() }, "not_found"),
            (Error::Precondition { path: path(), source: leaky() }, "precondition"),
            (Error::AlreadyExists { path: path(), source: leaky() }, "already_exists"),
            (Error::PermissionDenied { path: path(), source: leaky() }, "permission_denied"),
            (Error::Unauthenticated { path: path(), source: leaky() }, "unauthenticated"),
            (
                Error::NotImplemented {
                    operation: path(),
                    implementer: leaky().to_string(),
                },
                "not_implemented",
            ),
            (
                Error::InvalidPath {
                    source: object_store::path::Error::EmptySegment { path: path() },
                },
                "unparsable_key",
            ),
            (Error::Generic { store: "S3", source: leaky() }, "other"),
            (http(HttpErrorKind::Timeout), "timeout"),
            (http(HttpErrorKind::Connect), "transport"),
            // reqwest gives `Unknown` for an error that it cannot sort. The request can be sent.
            (http(HttpErrorKind::Unknown), "transport"),
        ]
    }

    #[test]
    fn each_kind_maps_to_its_class() {
        for (e, want) in all_kinds() {
            assert_eq!(class(&e), want, "{e:?}");
        }
    }

    #[test]
    fn no_class_text_holds_the_path_or_the_uri() {
        for (e, _) in all_kinds() {
            let text = store_text("s", &e);
            assert!(!text.contains(SECRET), "{text}");
            assert!(text.starts_with("s ("), "{text}");
        }
    }

    #[test]
    fn a_generic_status_is_kept_as_digits() {
        let e = Error::Generic {
            store: "S3",
            source: Box::new(Leaky(format!("Server returned non-2xx status code: 501 Not Implemented: {SECRET}"))),
        };
        assert_eq!(class(&e), "status=501");
        assert!(!store_text("s", &e).contains(SECRET));
    }

    #[test]
    fn only_timeout_and_transport_are_uncertain() {
        for (e, want) in all_kinds() {
            assert_eq!(is_uncertain(&e), matches!(want, "timeout" | "transport"), "{want}");
        }
    }

    fn status(code: &str) -> Error {
        Error::Generic {
            store: "S3",
            source: Box::new(Leaky(format!("Server returned non-2xx status code: {code} Reason: {SECRET}"))),
        }
    }

    #[test]
    fn a_5xx_other_than_501_is_uncertain() {
        // A 5xx does not prove that the write stopped. A 501 and a 4xx refusal do.
        let rows = [
            ("500", true),
            ("502", true),
            ("503", true),
            ("504", true),
            ("501", false),
            ("403", false),
            ("409", false),
            ("412", false),
        ];
        for (code, want) in rows {
            assert_eq!(is_uncertain(&status(code)), want, "{code}");
        }
    }

    #[test]
    fn refusals() {
        let kinds = all_kinds();
        let is = |i: usize| is_refusal(&kinds[i].0);
        assert!(is(1), "Precondition");
        assert!(is(2), "AlreadyExists");
        assert!(!is(0), "NotFound");
        assert!(!is(7), "Generic");
    }
}
