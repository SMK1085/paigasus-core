// SPDX-License-Identifier: Apache-2.0

//! The memory commands as pure functions over the workspace. The tool only parses the JSON.

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;

use super::text::{cut_view, format_size, number_lines, snippet, view_lines};
use super::{CapError, MemoryDir, MemoryPath, MemoryWorkspace, ParsedToolPath, PathError, WorkspaceError, parse_tool_path};

const COMMAND_NAMES: &str = "view, create, str_replace, insert, delete and rename";

/// One memory command with its arguments, as the model sends it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum MemoryCommand {
    /// Show a directory or a file.
    View {
        /// The tool path.
        path: String,
        /// The first and the last line to show. `-1` as the last line means the end.
        #[serde(default)]
        view_range: Option<(i64, i64)>,
    },
    /// Create a file, or overwrite it.
    Create {
        /// The tool path of the file.
        path: String,
        /// The content.
        file_text: String,
    },
    /// Replace one occurrence of a text.
    StrReplace {
        /// The tool path of the file.
        path: String,
        /// The text to replace. It must occur once.
        old_str: String,
        /// The new text. A missing value deletes `old_str`.
        #[serde(default)]
        new_str: Option<String>,
    },
    /// Insert a text after a line.
    Insert {
        /// The tool path of the file.
        path: String,
        /// The line after which the text goes. `0` is the start.
        insert_line: i64,
        /// The text to insert.
        insert_text: String,
    },
    /// Delete a file, or a directory and the files under it.
    Delete {
        /// The tool path.
        path: String,
    },
    /// Rename a file or a directory.
    Rename {
        /// The tool path of the source.
        old_path: String,
        /// The tool path of the destination.
        new_path: String,
    },
}

/// A refusal or a failure of a command. The `Display` text is for the model. The store cause, when
/// there is one, is kept apart for the host to log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCommandError {
    text: String,
    cause: Option<String>,
}

impl MemoryCommandError {
    fn new(text: impl Into<String>) -> Self {
        MemoryCommandError { text: text.into(), cause: None }
    }

    /// The store cause for the log. It is not for the model.
    #[must_use]
    pub fn cause(&self) -> Option<&str> {
        self.cause.as_deref()
    }
}

impl fmt::Display for MemoryCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl std::error::Error for MemoryCommandError {}

impl From<WorkspaceError> for MemoryCommandError {
    fn from(e: WorkspaceError) -> Self {
        let cause = match &e {
            WorkspaceError::Store(c) => Some(c.clone()),
            _ => None,
        };
        MemoryCommandError { text: e.to_string(), cause }
    }
}

impl From<PathError> for MemoryCommandError {
    fn from(e: PathError) -> Self {
        MemoryCommandError::new(e.to_string())
    }
}

impl From<CapError> for MemoryCommandError {
    fn from(e: CapError) -> Self {
        MemoryCommandError::new(e.to_string())
    }
}

/// Parse the arguments of a tool call into a command. An unknown command, a bad value or a missing
/// argument gives a text that names the six commands.
pub fn parse_command(args: serde_json::Value) -> Result<MemoryCommand, MemoryCommandError> {
    serde_json::from_value(args).map_err(|e| MemoryCommandError::new(format!("Error: The memory command is not valid: {e}. The commands are {COMMAND_NAMES}.")))
}

/// Run one command on the workspace. A sealed workspace refuses every command.
pub async fn execute(ws: &mut MemoryWorkspace, cmd: MemoryCommand) -> Result<String, MemoryCommandError> {
    if ws.is_sealed() {
        return Err(WorkspaceError::Sealed.into());
    }
    match cmd {
        MemoryCommand::View { path, view_range } => view(ws, &path, view_range).await,
        MemoryCommand::Create { path, file_text } => create(ws, &path, file_text).await,
        MemoryCommand::StrReplace { path, old_str, new_str } => str_replace(ws, &path, &old_str, new_str.as_deref().unwrap_or("")).await,
        MemoryCommand::Insert { path, insert_line, insert_text } => insert(ws, &path, insert_line, &insert_text).await,
        MemoryCommand::Delete { .. } => Err(not_available("delete")),
        MemoryCommand::Rename { .. } => Err(not_available("rename")),
    }
}

fn not_available(command: &str) -> MemoryCommandError {
    MemoryCommandError::new(format!("Error: the {command} command is not available yet."))
}

fn missing_view(tool_path: &str) -> MemoryCommandError {
    MemoryCommandError::new(format!("The path {tool_path} does not exist. Please provide a valid path."))
}

fn missing_edit(tool_path: &str) -> MemoryCommandError {
    MemoryCommandError::new(format!("Error: The path {tool_path} does not exist. Please provide a valid path."))
}

/// The file of a path argument of a file command.
fn file_arg(raw: &str) -> Result<MemoryPath, MemoryCommandError> {
    let ParsedToolPath { file, trailing_slash, .. } = parse_tool_path(raw)?;
    match file {
        Some(f) => Ok(f),
        None if trailing_slash => Err(MemoryCommandError::new(format!("Error: The path {raw} ends with / and is a directory path. Give the path of a file."))),
        None => Err(PathError::RootNotAFile.into()),
    }
}

async fn view(ws: &mut MemoryWorkspace, raw: &str, range: Option<(i64, i64)>) -> Result<String, MemoryCommandError> {
    let parsed = parse_tool_path(raw)?;
    if let Some(file) = &parsed.file
        && let Some(content) = ws.read(file).await?
    {
        return view_file(file, &content, range);
    }
    view_dir(ws, &parsed.dir, raw).await
}

fn view_file(file: &MemoryPath, content: &str, range: Option<(i64, i64)>) -> Result<String, MemoryCommandError> {
    let lines = view_lines(content);
    let n = lines.len();
    let (start, end) = match range {
        None => (1, n),
        Some((s, e)) => {
            let bad = s < 1 || (e != -1 && s > e) || usize::try_from(s).is_ok_and(|s| s > n);
            if bad {
                return Err(MemoryCommandError::new(format!("Error: Invalid `view_range` {s}..{e}. The file has {n} lines.")));
            }
            let end = if e == -1 { n } else { usize::try_from(e).map_or(n, |e| e.min(n)) };
            (usize::try_from(s).unwrap_or(1), end)
        }
    };
    let body = cut_view(number_lines(&lines[start - 1..end], start));
    Ok(format!("Here's the content of {} with line numbers:\n{body}", file.to_tool_path()))
}

async fn view_dir(ws: &mut MemoryWorkspace, dir: &MemoryDir, raw: &str) -> Result<String, MemoryCommandError> {
    let files = ws.list(dir).await?;
    if files.is_empty() && !dir.is_root() {
        return Err(missing_view(raw.strip_suffix('/').unwrap_or(raw)));
    }
    let base = if dir.is_root() { 0 } else { dir.as_str().split('/').count() - 1 };
    // Each entry is keyed by its segments, so the order is the tree order.
    let mut entries: BTreeMap<Vec<&str>, u64> = BTreeMap::new();
    let mut total = 0;
    for (path, size) in &files {
        total += size;
        let segments: Vec<&str> = path.as_str().split('/').skip(1).collect();
        let rel = &segments[base..];
        if rel.contains(&"node_modules") {
            continue;
        }
        for depth in 1..=rel.len().min(2) {
            *entries.entry(rel[..depth].to_vec()).or_insert(0) += size;
        }
    }
    entries.insert(Vec::new(), total);
    let mut out = format!(
        "Here're the files and directories up to 2 levels deep in {}, excluding hidden items and node_modules:",
        dir.to_tool_path()
    );
    for (rel, size) in &entries {
        let tool_path = if rel.is_empty() {
            dir.to_tool_path()
        } else {
            format!("{}/{}", dir.to_tool_path(), rel.join("/"))
        };
        out.push_str(&format!("\n{}\t{tool_path}", format_size(*size)));
    }
    Ok(out)
}

async fn create(ws: &mut MemoryWorkspace, raw: &str, text: String) -> Result<String, MemoryCommandError> {
    let file = file_arg(raw)?;
    let tool = file.to_tool_path();
    if let Some((inner, _)) = ws.list(&file.as_dir()).await?.first() {
        return Err(MemoryCommandError::new(format!(
            "Error: Cannot create the file {tool}. The path is a directory and the file {} is in it.",
            inner.to_tool_path()
        )));
    }
    let mut prefix = String::new();
    let segments: Vec<&str> = file.as_str().split('/').skip(1).collect();
    for segment in &segments[..segments.len() - 1] {
        prefix.push('/');
        prefix.push_str(segment);
        let ancestor = MemoryPath::parse(&prefix)?;
        if ws.read(&ancestor).await?.is_some() {
            return Err(MemoryCommandError::new(format!(
                "Error: Cannot create the file {tool}. The file {} exists and a file cannot be in a file.",
                ancestor.to_tool_path()
            )));
        }
    }
    ws.write(&file, text).await?;
    Ok(format!("File created successfully at: {tool}"))
}

async fn str_replace(ws: &mut MemoryWorkspace, raw: &str, old: &str, new: &str) -> Result<String, MemoryCommandError> {
    let file = file_arg(raw)?;
    let tool = file.to_tool_path();
    let Some(content) = ws.read(&file).await? else {
        return Err(missing_edit(&tool));
    };
    if old.is_empty() {
        return Err(MemoryCommandError::new("Error: old_str must not be empty."));
    }
    let line_of = |at: usize| content[..at].matches('\n').count();
    let hits: Vec<usize> = content.match_indices(old).map(|(i, _)| i).collect();
    match hits.as_slice() {
        [] => Err(MemoryCommandError::new(format!("No replacement was performed, old_str `{old}` did not appear verbatim in {tool}."))),
        [at] => {
            let mut edited = String::with_capacity(content.len() + new.len());
            edited.push_str(&content[..*at]);
            edited.push_str(new);
            edited.push_str(&content[at + old.len()..]);
            let first_line = line_of(*at);
            ws.write(&file, edited.clone()).await?;
            Ok(format!("The memory file has been edited.\n{}", snippet(&edited, first_line, new.matches('\n').count())))
        }
        many => {
            let lines: Vec<String> = many.iter().map(|at| (line_of(*at) + 1).to_string()).collect();
            Err(MemoryCommandError::new(format!(
                "No replacement was performed. Multiple occurrences of old_str `{old}` in lines: {}. Please ensure it is unique",
                lines.join(", ")
            )))
        }
    }
}

async fn insert(ws: &mut MemoryWorkspace, raw: &str, line: i64, text: &str) -> Result<String, MemoryCommandError> {
    let file = file_arg(raw)?;
    let tool = file.to_tool_path();
    let Some(content) = ws.read(&file).await? else {
        return Err(MemoryCommandError::new(format!("Error: The path {tool} does not exist")));
    };
    let mut lines: Vec<&str> = content.split('\n').collect();
    let n_lines = lines.len();
    let at = usize::try_from(line).ok().filter(|at| *at <= n_lines).ok_or_else(|| {
        MemoryCommandError::new(format!(
            "Error: Invalid `insert_line` parameter: {line}. It should be within the range of lines of the file: [0, {n_lines}]"
        ))
    })?;
    let inserted: Vec<&str> = text.strip_suffix('\n').unwrap_or(text).split('\n').collect();
    lines.splice(at..at, inserted);
    ws.write(&file, lines.join("\n")).await?;
    Ok(format!("The file {tool} has been edited."))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::memory::MemoryCaps;
    use crate::memory::conformance::fresh_scope;
    use crate::memory::fake::InMemoryMemoryStore;

    async fn ws_with(files: &[(&str, &str)]) -> MemoryWorkspace {
        let mut ws = MemoryWorkspace::new(Arc::new(InMemoryMemoryStore::default()), fresh_scope(), MemoryCaps::default());
        for (p, c) in files {
            ws.write(&MemoryPath::parse(p).unwrap(), (*c).to_string()).await.unwrap();
        }
        ws
    }

    async fn run(ws: &mut MemoryWorkspace, v: serde_json::Value) -> Result<String, String> {
        let cmd = parse_command(v).map_err(|e| e.to_string())?;
        execute(ws, cmd).await.map_err(|e| e.to_string())
    }

    async fn content(ws: &mut MemoryWorkspace, p: &str) -> Option<String> {
        ws.read(&MemoryPath::parse(p).unwrap()).await.unwrap()
    }

    #[tokio::test]
    async fn view_empty_root() {
        let mut ws = ws_with(&[]).await;
        let out = run(&mut ws, json!({"command":"view","path":"/memories"})).await.unwrap();
        assert_eq!(
            out,
            "Here're the files and directories up to 2 levels deep in /memories, excluding hidden items and node_modules:\n0.0K\t/memories"
        );
    }

    #[tokio::test]
    async fn view_dir_two_levels() {
        let mut ws = ws_with(&[("/a.md", &"x".repeat(512)), ("/n/b.md", "b"), ("/n/x/c.md", "c")]).await;
        let out = run(&mut ws, json!({"command":"view","path":"/memories/"})).await.unwrap();
        let rows: Vec<&str> = out.lines().skip(1).map(|l| l.split('\t').nth(1).unwrap()).collect();
        assert_eq!(rows, ["/memories", "/memories/a.md", "/memories/n", "/memories/n/b.md", "/memories/n/x"]);
        assert!(out.contains("0.5K\t/memories/a.md"));
        let sub = run(&mut ws, json!({"command":"view","path":"/memories/n"})).await.unwrap();
        assert!(sub.starts_with("Here're the files and directories up to 2 levels deep in /memories/n,"));
        assert!(sub.contains("\t/memories/n\n") && sub.contains("\t/memories/n/x/c.md"));
    }

    #[tokio::test]
    async fn view_missing_dir() {
        let mut ws = ws_with(&[]).await;
        let err = run(&mut ws, json!({"command":"view","path":"/memories/nothing"})).await.unwrap_err();
        assert_eq!(err, "The path /memories/nothing does not exist. Please provide a valid path.");
    }

    #[tokio::test]
    async fn view_file_numbers() {
        let mut ws = ws_with(&[("/a", "a\nb\n"), ("/e", "")]).await;
        let out = run(&mut ws, json!({"command":"view","path":"/memories/a"})).await.unwrap();
        assert_eq!(out, "Here's the content of /memories/a with line numbers:\n     1\ta\n     2\tb");
        let out = run(&mut ws, json!({"command":"view","path":"/memories/e"})).await.unwrap();
        assert_eq!(out, "Here's the content of /memories/e with line numbers:\n     1\t");
    }

    #[tokio::test]
    async fn view_range_edges() {
        let mut ws = ws_with(&[("/a", "a\nb\nc")]).await;
        let v = |r: serde_json::Value| json!({"command":"view","path":"/memories/a","view_range":r});
        assert!(run(&mut ws, v(json!([2, -1]))).await.unwrap().ends_with("     2\tb\n     3\tc"));
        assert!(run(&mut ws, v(json!([1, 99]))).await.unwrap().ends_with("     3\tc"));
        for r in [json!([0, 1]), json!([3, 2]), json!([9, 9])] {
            let err = run(&mut ws, v(r.clone())).await.unwrap_err();
            let (s, e) = (r[0].as_i64().unwrap(), r[1].as_i64().unwrap());
            assert_eq!(err, format!("Error: Invalid `view_range` {s}..{e}. The file has 3 lines."));
        }
    }

    #[tokio::test]
    async fn view_cuts_long_files() {
        let mut ws = ws_with(&[("/a", &"y".repeat(20_000))]).await;
        let out = run(&mut ws, json!({"command":"view","path":"/memories/a"})).await.unwrap();
        assert!(out.ends_with("\n[The view is cut at 16,000 characters. Use view_range to read the rest.]"));
        assert!(out.chars().count() < 16_300);
    }

    #[tokio::test]
    async fn create_new_and_overwrite() {
        let mut ws = ws_with(&[]).await;
        let ok = "File created successfully at: /memories/a.md";
        assert_eq!(run(&mut ws, json!({"command":"create","path":"/memories/a.md","file_text":"one"})).await.unwrap(), ok);
        assert_eq!(run(&mut ws, json!({"command":"create","path":"/memories/a.md","file_text":"two"})).await.unwrap(), ok);
        assert_eq!(content(&mut ws, "/a.md").await.as_deref(), Some("two"));
    }

    #[tokio::test]
    async fn create_refuses_file_dir_clash() {
        let mut ws = ws_with(&[("/notes/index.md", "i"), ("/a.md", "a")]).await;
        let err = run(&mut ws, json!({"command":"create","path":"/memories/notes","file_text":"x"})).await.unwrap_err();
        assert!(err.contains("/memories/notes") && err.contains("/memories/notes/index.md"), "{err}");
        let err = run(&mut ws, json!({"command":"create","path":"/memories/a.md/b.md","file_text":"x"})).await.unwrap_err();
        assert!(err.contains("/memories/a.md/b.md") && err.contains("/memories/a.md "), "{err}");
        assert_eq!(content(&mut ws, "/notes").await, None);
        assert_eq!(content(&mut ws, "/a.md/b.md").await, None);
    }

    #[tokio::test]
    async fn str_replace_cases() {
        let mut ws = ws_with(&[("/a", "one\ntwo\nthree\ntwo"), ("/crlf", "x\r\ny\r\n"), ("/o", "aaa")]).await;
        let sr = |p: &str, old: &str| json!({"command":"str_replace","path":format!("/memories{p}"),"old_str":old,"new_str":"Z"});
        assert_eq!(
            run(&mut ws, sr("/a", "nope")).await.unwrap_err(),
            "No replacement was performed, old_str `nope` did not appear verbatim in /memories/a."
        );
        assert_eq!(
            run(&mut ws, sr("/a", "two")).await.unwrap_err(),
            "No replacement was performed. Multiple occurrences of old_str `two` in lines: 2, 4. Please ensure it is unique"
        );
        assert_eq!(run(&mut ws, sr("/a", "")).await.unwrap_err(), "Error: old_str must not be empty.");
        assert_eq!(
            run(&mut ws, sr("/missing", "x")).await.unwrap_err(),
            "Error: The path /memories/missing does not exist. Please provide a valid path."
        );
        let ok = run(&mut ws, sr("/a", "three")).await.unwrap();
        assert_eq!(ok, "The memory file has been edited.\n     1\tone\n     2\ttwo\n     3\tZ\n     4\ttwo");
        assert_eq!(content(&mut ws, "/a").await.as_deref(), Some("one\ntwo\nZ\ntwo"));
        // A missing new_str deletes old_str.
        run(&mut ws, json!({"command":"str_replace","path":"/memories/a","old_str":"one\n"})).await.unwrap();
        assert_eq!(content(&mut ws, "/a").await.as_deref(), Some("two\nZ\ntwo"));
        // CRLF stays.
        run(&mut ws, sr("/crlf", "y")).await.unwrap();
        assert_eq!(content(&mut ws, "/crlf").await.as_deref(), Some("x\r\nZ\r\n"));
        // Overlapping matches count once.
        run(&mut ws, sr("/o", "aa")).await.unwrap();
        assert_eq!(content(&mut ws, "/o").await.as_deref(), Some("Za"));
    }

    #[tokio::test]
    async fn insert_cases() {
        let ins = |p: &str, n: i64, t: &str| json!({"command":"insert","path":format!("/memories{p}"),"insert_line":n,"insert_text":t});
        let mut ws = ws_with(&[("/a", "a\nb"), ("/c", "a\nb\n")]).await;
        assert_eq!(run(&mut ws, ins("/a", 0, "X")).await.unwrap(), "The file /memories/a has been edited.");
        assert_eq!(content(&mut ws, "/a").await.as_deref(), Some("X\na\nb"));
        run(&mut ws, ins("/a", 3, "Y\n")).await.unwrap();
        assert_eq!(content(&mut ws, "/a").await.as_deref(), Some("X\na\nb\nY"));
        let err = run(&mut ws, ins("/a", 5, "Z")).await.unwrap_err();
        assert_eq!(err, "Error: Invalid `insert_line` parameter: 5. It should be within the range of lines of the file: [0, 4]");
        assert!(run(&mut ws, ins("/a", -1, "Z")).await.is_err());
        // "a\nb\n" has 3 lines.
        assert!(run(&mut ws, ins("/c", 4, "Z")).await.unwrap_err().ends_with("[0, 3]"));
        run(&mut ws, ins("/c", 3, "Z")).await.unwrap();
        assert_eq!(run(&mut ws, ins("/nope", 0, "Z")).await.unwrap_err(), "Error: The path /memories/nope does not exist");
    }

    #[tokio::test]
    async fn directory_given_to_file_command() {
        let mut ws = ws_with(&[("/n/a", "a")]).await;
        let err = run(&mut ws, json!({"command":"str_replace","path":"/memories/n","old_str":"a","new_str":"b"})).await.unwrap_err();
        assert_eq!(err, "Error: The path /memories/n does not exist. Please provide a valid path.");
        let err = run(&mut ws, json!({"command":"insert","path":"/memories/n","insert_line":0,"insert_text":"b"})).await.unwrap_err();
        assert_eq!(err, "Error: The path /memories/n does not exist");
    }

    #[tokio::test]
    async fn parse_command_errors() {
        for v in [json!({"command":"nope"}), json!({}), json!({"command":"create","path":"/memories/a"})] {
            let err = parse_command(v).unwrap_err().to_string();
            assert!(err.contains("view, create, str_replace, insert, delete and rename"), "{err}");
        }
        let err = parse_command(json!({"command":"create","path":"/memories/a"})).unwrap_err().to_string();
        assert!(err.contains("file_text"), "{err}");
    }

    #[tokio::test]
    async fn sealed_and_deferred_commands() {
        let mut ws = ws_with(&[("/a", "a")]).await;
        assert_eq!(
            run(&mut ws, json!({"command":"delete","path":"/memories/a"})).await.unwrap_err(),
            "Error: the delete command is not available yet."
        );
        assert_eq!(
            run(&mut ws, json!({"command":"rename","old_path":"/memories/a","new_path":"/memories/b"})).await.unwrap_err(),
            "Error: the rename command is not available yet."
        );
        ws.take_changes();
        for v in [json!({"command":"view","path":"/memories"}), json!({"command":"delete","path":"/memories/a"})] {
            assert_eq!(run(&mut ws, v).await.unwrap_err(), "The run is ending. Memory is read-only now.");
        }
    }
}
