// SPDX-License-Identifier: Apache-2.0

//! Validated memory paths, and the mapping to and from the tool form (`/memories/...`).

use std::fmt;

use serde::{Deserialize, Serialize};

/// The root of every path as the model sees it.
pub const TOOL_ROOT: &str = "/memories";

const MAX_BYTES: usize = 256;
const MAX_SEGMENTS: usize = 8;
const MAX_SEGMENT_BYTES: usize = 64;

/// A path refused by a rule. The `Display` text is for the model.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    /// The tool path is not under `/memories`.
    #[error("The path {path} is not allowed. Every path must be /memories or start with /memories/.")]
    OutsideMemories { path: String },
    /// The path breaks a path rule.
    #[error("The path {path} is not allowed: {reason}.")]
    Invalid { path: String, reason: &'static str },
    /// A file path names the root, which is a directory.
    #[error("The path /memories is the root directory. It is not a file.")]
    RootNotAFile,
}

/// The length refusal of a store path.
const TOO_LONG: &str = "a path has at most 256 bytes";
/// The length refusal of a tool path: the store limit plus the bytes of `TOOL_ROOT`.
const TOOL_TOO_LONG: &str = "a path has at most 265 bytes including /memories";

/// The reason as the model must read it for a tool path. Only the length rule differs, because
/// the tool path holds the `/memories` prefix and the store path does not.
pub(super) fn tool_reason(reason: &'static str) -> &'static str {
    if reason == TOO_LONG { TOOL_TOO_LONG } else { reason }
}

fn invalid(path: &str, reason: &'static str) -> PathError {
    PathError::Invalid { path: path.to_string(), reason }
}

/// Check a path that is not the root. The root `/` is the caller's case.
fn validate_non_root(path: &str) -> Result<(), PathError> {
    if path.len() > MAX_BYTES {
        return Err(invalid(path, TOO_LONG));
    }
    let Some(rest) = path.strip_prefix('/') else {
        return Err(invalid(path, "a path starts with /"));
    };
    let mut count = 0;
    for segment in rest.split('/') {
        count += 1;
        if segment.is_empty() {
            return Err(invalid(path, "a path has no empty segment and no trailing /"));
        }
        if segment == "." || segment == ".." {
            return Err(invalid(path, "a path has no . or .. segment"));
        }
        if segment.starts_with('.') {
            return Err(invalid(path, "a segment does not start with ."));
        }
        if segment.len() > MAX_SEGMENT_BYTES {
            return Err(invalid(path, "a segment has at most 64 bytes"));
        }
        if !segment.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')) {
            return Err(invalid(path, "a segment uses only the characters A-Z a-z 0-9 . _ -"));
        }
    }
    if count > MAX_SEGMENTS {
        return Err(invalid(path, "a path has at most 8 segments"));
    }
    Ok(())
}

/// A directory path. The root `/` is valid.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MemoryDir(String);

impl MemoryDir {
    /// Parse a directory path.
    pub fn parse(raw: &str) -> Result<Self, PathError> {
        if raw == "/" {
            return Ok(Self::root());
        }
        validate_non_root(raw)?;
        Ok(MemoryDir(raw.to_string()))
    }

    /// The root directory.
    #[must_use]
    pub fn root() -> Self {
        MemoryDir("/".to_string())
    }

    /// The path as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True for the root.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.0 == "/"
    }

    /// True when the file is under this directory at any depth. The test is on segments, never on
    /// a string prefix.
    #[must_use]
    pub fn contains(&self, path: &MemoryPath) -> bool {
        self.is_root() || path.0.strip_prefix(self.0.as_str()).is_some_and(|rest| rest.starts_with('/'))
    }

    /// True when the other directory is this directory or is under it at any depth.
    #[must_use]
    pub fn contains_dir(&self, other: &MemoryDir) -> bool {
        self.is_root() || other.0 == self.0 || other.0.strip_prefix(self.0.as_str()).is_some_and(|rest| rest.starts_with('/'))
    }

    /// The path as the model sees it.
    #[must_use]
    pub fn to_tool_path(&self) -> String {
        if self.is_root() { TOOL_ROOT.to_string() } else { format!("{TOOL_ROOT}{}", self.0) }
    }
}

impl TryFrom<String> for MemoryDir {
    type Error = PathError;
    fn try_from(raw: String) -> Result<Self, PathError> {
        MemoryDir::parse(&raw)
    }
}

impl From<MemoryDir> for String {
    fn from(dir: MemoryDir) -> String {
        dir.0
    }
}

impl fmt::Display for MemoryDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A file path. The root is not valid.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MemoryPath(String);

impl MemoryPath {
    /// Parse a file path. The root `/` is refused.
    pub fn parse(raw: &str) -> Result<Self, PathError> {
        if raw == "/" {
            return Err(PathError::RootNotAFile);
        }
        validate_non_root(raw)?;
        Ok(MemoryPath(raw.to_string()))
    }

    /// The path as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The same path as a directory.
    #[must_use]
    pub fn as_dir(&self) -> MemoryDir {
        MemoryDir(self.0.clone())
    }

    /// The directory that holds the file.
    #[must_use]
    pub fn parent(&self) -> MemoryDir {
        match self.0.rfind('/') {
            Some(0) | None => MemoryDir::root(),
            Some(i) => MemoryDir(self.0[..i].to_string()),
        }
    }

    /// Move the file from one directory to another and check the result against the path rules.
    /// The file must be under `from`.
    pub fn rebase(&self, from: &MemoryDir, to: &MemoryDir) -> Result<MemoryPath, PathError> {
        if !from.contains(self) {
            return Err(invalid(&self.0, "the path is not under the source directory"));
        }
        let rest = if from.is_root() { self.0.as_str() } else { &self.0[from.0.len()..] };
        let joined = if to.is_root() { rest.to_string() } else { format!("{}{rest}", to.0) };
        MemoryPath::parse(&joined)
    }

    /// The path as the model sees it.
    #[must_use]
    pub fn to_tool_path(&self) -> String {
        format!("{TOOL_ROOT}{}", self.0)
    }
}

impl TryFrom<String> for MemoryPath {
    type Error = PathError;
    fn try_from(raw: String) -> Result<Self, PathError> {
        MemoryPath::parse(&raw)
    }
}

impl From<MemoryPath> for String {
    fn from(path: MemoryPath) -> String {
        path.0
    }
}

impl fmt::Display for MemoryPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A path as the model sends it, after parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedToolPath {
    /// The path as a directory.
    pub dir: MemoryDir,
    /// The path as a file. It is `None` for the root and for any form with a trailing `/`.
    pub file: Option<MemoryPath>,
    /// True when the raw path ended with `/`.
    pub trailing_slash: bool,
}

/// Parse a path as the model sends it. It must be `/memories` or start with `/memories/`.
pub fn parse_tool_path(raw: &str) -> Result<ParsedToolPath, PathError> {
    let outside = || PathError::OutsideMemories { path: raw.to_string() };
    let rest = raw.strip_prefix(TOOL_ROOT).ok_or_else(outside)?;
    if rest.is_empty() {
        return Ok(ParsedToolPath {
            dir: MemoryDir::root(),
            file: None,
            trailing_slash: false,
        });
    }
    if !rest.starts_with('/') {
        return Err(outside());
    }
    // Report a refusal with the tool path, not with the store path.
    let rename = |e: PathError| match e {
        PathError::Invalid { reason, .. } => invalid(raw, tool_reason(reason)),
        other => other,
    };
    if let Some(body) = rest.strip_suffix('/') {
        // Exactly one trailing slash is allowed. "/memories//" has a body of "/".
        if body == "/" {
            return Err(invalid(raw, "a path has no empty segment and no trailing /"));
        }
        let dir = if body.is_empty() { MemoryDir::root() } else { MemoryDir::parse(body).map_err(rename)? };
        return Ok(ParsedToolPath {
            dir,
            file: None,
            trailing_slash: true,
        });
    }
    let file = MemoryPath::parse(rest).map_err(rename)?;
    Ok(ParsedToolPath {
        dir: file.as_dir(),
        file: Some(file),
        trailing_slash: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tool_length_refusal_counts_the_tool_root() {
        assert_eq!(TOOL_TOO_LONG, format!("a path has at most {} bytes including {TOOL_ROOT}", MAX_BYTES + TOOL_ROOT.len()));
        assert_eq!(TOO_LONG, format!("a path has at most {MAX_BYTES} bytes"));
        let long_257 = format!("/{}/{}/{}/{}/b", "a".repeat(64), "a".repeat(64), "a".repeat(64), "a".repeat(59));
        let tool = format!("{TOOL_ROOT}{long_257}");
        assert_eq!(tool.len(), 266);
        let err = parse_tool_path(&tool).unwrap_err();
        assert_eq!(err.to_string(), format!("The path {tool} is not allowed: a path has at most 265 bytes including /memories."));
        assert_eq!(tool_reason("a path starts with /"), "a path starts with /");
    }

    #[test]
    fn store_path_rules() {
        let nine = "/a/b/c/d/e/f/g/h/i";
        let long_257 = format!("/{}/{}/{}/{}/b", "a".repeat(64), "a".repeat(64), "a".repeat(64), "a".repeat(59));
        assert_eq!(long_257.len(), 257);
        let seg_65 = format!("/{}", "a".repeat(65));
        let bad = [
            "/",
            "x",
            "/..",
            "/a/../b",
            "/./x",
            "/a//b",
            "/a/",
            "/%2e%2e/x",
            "/a\\b",
            "/a\0b",
            "/.hidden",
            "/\u{FF0E}x",
            "/a b",
            "/é",
            long_257.as_str(),
            nine,
            seg_65.as_str(),
        ];
        for raw in bad {
            assert!(MemoryPath::parse(raw).is_err(), "{raw:?} must be refused");
        }
        let long_256 = format!("/{}/{}/{}/{}/b", "a".repeat(64), "a".repeat(64), "a".repeat(64), "a".repeat(58));
        assert_eq!(long_256.len(), 256);
        for raw in ["/instructions.md", "/tasks/t-1_x.md", "/a/b/c/d/e/f/g/h", long_256.as_str()] {
            assert!(MemoryPath::parse(raw).is_ok(), "{raw:?} must be accepted");
        }
        assert_eq!(MemoryPath::parse("/").unwrap_err(), PathError::RootNotAFile);
        assert!(MemoryDir::parse("/").is_ok());
        assert!(MemoryDir::parse("/a/").is_err());
    }

    #[test]
    fn tool_path_rules() {
        let p = parse_tool_path("/memories").unwrap();
        assert!(p.dir.is_root() && p.file.is_none() && !p.trailing_slash);
        let p = parse_tool_path("/memories/").unwrap();
        assert!(p.dir.is_root() && p.file.is_none() && p.trailing_slash);
        let p = parse_tool_path("/memories/tasks/").unwrap();
        assert_eq!(p.dir.as_str(), "/tasks");
        assert!(p.file.is_none() && p.trailing_slash);
        let p = parse_tool_path("/memories/tasks/t1.md").unwrap();
        assert_eq!(p.dir.as_str(), "/tasks/t1.md");
        assert_eq!(p.file.unwrap().as_str(), "/tasks/t1.md");
        assert!(!p.trailing_slash);
        for raw in ["/memoriesX/a", "memories/a", "/a", ""] {
            assert!(matches!(parse_tool_path(raw), Err(PathError::OutsideMemories { .. })), "{raw:?}");
        }
        for raw in ["/memories/../x", "/memories//a", "/memories/a//", "/memories//"] {
            assert!(matches!(parse_tool_path(raw), Err(PathError::Invalid { .. })), "{raw:?}");
        }
        assert_eq!(MemoryPath::parse("/tasks/t1.md").unwrap().to_tool_path(), "/memories/tasks/t1.md");
        assert_eq!(MemoryDir::root().to_tool_path(), "/memories");
        assert_eq!(MemoryDir::parse("/tasks").unwrap().to_tool_path(), "/memories/tasks");
    }

    #[test]
    fn refusal_text_names_the_tool_path() {
        let text = parse_tool_path("/memories/../x").unwrap_err().to_string();
        assert!(text.contains("/memories/../x"), "{text}");
        assert!(parse_tool_path("/a").unwrap_err().to_string().contains("/memories"));
    }

    #[test]
    fn dir_contains_is_segment_based() {
        let a = MemoryDir::parse("/a").unwrap();
        for ok in ["/a/x", "/a/b/c"] {
            assert!(a.contains(&MemoryPath::parse(ok).unwrap()), "{ok}");
        }
        for no in ["/ab/x", "/a"] {
            assert!(!a.contains(&MemoryPath::parse(no).unwrap()), "{no}");
        }
        assert!(MemoryDir::root().contains(&MemoryPath::parse("/a").unwrap()));
        assert!(a.contains_dir(&MemoryDir::parse("/a/b").unwrap()));
        assert!(!a.contains_dir(&MemoryDir::parse("/ab").unwrap()));
        assert!(MemoryDir::root().contains_dir(&a));
    }

    #[test]
    fn parent_and_serde() {
        assert!(MemoryPath::parse("/x.md").unwrap().parent().is_root());
        assert_eq!(MemoryPath::parse("/a/b/x.md").unwrap().parent().as_str(), "/a/b");
        assert_eq!(serde_json::to_string(&MemoryPath::parse("/a").unwrap()).unwrap(), "\"/a\"");
        assert!(serde_json::from_str::<MemoryPath>("\"/a/../b\"").is_err());
        assert!(serde_json::from_str::<MemoryDir>("\"/\"").is_ok());
        assert!(serde_json::from_str::<MemoryDir>("\"/a/\"").is_err());
    }

    #[test]
    fn rebase_checks_the_result() {
        let x = MemoryPath::parse("/a/x").unwrap();
        let a = MemoryDir::parse("/a").unwrap();
        let b = MemoryDir::parse("/b").unwrap();
        assert_eq!(x.rebase(&a, &b).unwrap().as_str(), "/b/x");
        assert_eq!(x.rebase(&a, &MemoryDir::root()).unwrap().as_str(), "/x");
        let deep = MemoryDir::parse("/1/2/3/4/5/6/7/8").unwrap();
        assert!(matches!(x.rebase(&a, &deep), Err(PathError::Invalid { .. })));
        assert!(matches!(x.rebase(&b, &a), Err(PathError::Invalid { .. })));
    }
}
