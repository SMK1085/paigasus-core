// SPDX-License-Identifier: Apache-2.0

//! Tool names (spec section 4.1): `<namespace>.<name>` or `<name>`.

/// Match keys that no tool may use. They are the two Cedar action groups (SMA-752).
pub const RESERVED_NAME_KEYS: &[&str] = &["invoketool", "unregistered"];

/// A parsed tool name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolName {
    /// The qualified name as given. It is shown to people only.
    pub display: String,
    /// The match key. It is the Cedar action id.
    pub name_key: String,
}

/// Why a tool name is not valid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    /// A part does not match `^[A-Za-z0-9_-]{1,64}$`. The message holds only that part.
    #[error("tool name part {part:?} does not match ^[A-Za-z0-9_-]{{1,64}}$")]
    Grammar { part: String },
    /// The name is `invoketool` or `unregistered` in any case.
    #[error("tool name is reserved")]
    Reserved,
}

/// The match key of a qualified name.
///
/// This is ASCII lowercase. On the ASCII name grammar it equals the kernel normaliser
/// (trim, NFC, lowercase, NFC), because trim and NFC do nothing for these characters.
/// PR 2 changes this function to call the kernel normaliser (D4).
pub fn name_key(qualified: &str) -> String {
    qualified.to_ascii_lowercase()
}

fn check_part(part: &str) -> Result<(), NameError> {
    let ok = !part.is_empty() && part.len() <= 64 && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if ok {
        Ok(())
    } else {
        // Echo at most 64 characters of the offending part.
        Err(NameError::Grammar {
            part: part.chars().take(64).collect(),
        })
    }
}

/// Parse a name from an optional namespace and a name.
pub fn parse_tool_name(namespace: Option<&str>, name: &str) -> Result<ToolName, NameError> {
    if let Some(ns) = namespace {
        check_part(ns)?;
    }
    check_part(name)?;
    let display = match namespace {
        Some(ns) => format!("{ns}.{name}"),
        None => name.to_owned(),
    };
    let key = name_key(&display);
    if RESERVED_NAME_KEYS.contains(&key.as_str()) {
        return Err(NameError::Reserved);
    }
    Ok(ToolName { display, name_key: key })
}

/// Parse a qualified name. It has at most one dot.
pub fn parse_qualified(qualified: &str) -> Result<ToolName, NameError> {
    let mut parts = qualified.split('.');
    let first = parts.next().unwrap_or("");
    match (parts.next(), parts.next()) {
        (None, _) => parse_tool_name(None, first),
        (Some(second), None) => parse_tool_name(Some(first), second),
        (Some(_), Some(_)) => Err(NameError::Grammar {
            part: qualified.chars().take(64).collect(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_namespaced_name_gets_a_lowercase_key() {
        assert_eq!(parse_tool_name(Some("GitHub"), "Search").unwrap().name_key, "github.search");
    }

    #[test]
    fn display_keeps_the_given_case() {
        assert_eq!(parse_qualified("GitHub.Search").unwrap().display, "GitHub.Search");
    }

    #[test]
    fn each_part_follows_the_openai_grammar() {
        for bad in ["", "a b", "é", &"x".repeat(65), "a/b"] {
            assert!(parse_tool_name(None, bad).is_err(), "{bad:?}");
            assert!(parse_tool_name(Some(bad), "ok").is_err(), "{bad:?} as namespace");
        }
        assert!(parse_tool_name(None, &"x".repeat(64)).is_ok());
        assert!(parse_tool_name(None, "a_b-C9").is_ok());
    }

    #[test]
    fn the_error_echoes_only_the_offending_part() {
        let err = parse_tool_name(Some("good"), "bad name").unwrap_err();
        assert_eq!(err, NameError::Grammar { part: "bad name".into() });
        assert!(!err.to_string().contains("good"));
    }

    #[test]
    fn reserved_names_are_rejected_in_any_case() {
        for n in ["InvokeTool", "UNREGISTERED", "invoketool"] {
            assert_eq!(parse_qualified(n).unwrap_err(), NameError::Reserved);
        }
    }

    #[test]
    fn a_reserved_word_inside_a_namespace_is_allowed() {
        assert_eq!(parse_qualified("gh.invoketool").unwrap().name_key, "gh.invoketool");
    }

    #[test]
    fn a_qualified_name_with_two_dots_is_rejected() {
        assert!(parse_qualified("a.b.c").is_err());
    }
}
