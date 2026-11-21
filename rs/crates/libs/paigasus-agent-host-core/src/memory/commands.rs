// SPDX-License-Identifier: Apache-2.0

//! The memory commands as pure functions over the workspace. The tool only parses the JSON.

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;

use super::path::tool_reason;
use super::text::{format_size, number_lines_cut, snippet, view_lines};
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
    kind: MemoryRefusalKind,
}

/// The class of a refusal, for a counter label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRefusalKind {
    /// The file changed in the store during the run.
    Conflict,
    /// A cap refused the change.
    Cap,
    /// The path or the arguments are not valid, or the target is missing.
    Path,
    /// The workspace is sealed.
    Sealed,
    /// The store failed.
    Store,
}

impl MemoryRefusalKind {
    /// The label value of the kind.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            MemoryRefusalKind::Conflict => "conflict",
            MemoryRefusalKind::Cap => "cap",
            MemoryRefusalKind::Path => "path",
            MemoryRefusalKind::Sealed => "sealed",
            MemoryRefusalKind::Store => "store",
        }
    }
}

impl MemoryCommandError {
    fn new(text: impl Into<String>) -> Self {
        MemoryCommandError {
            text: text.into(),
            cause: None,
            kind: MemoryRefusalKind::Path,
        }
    }

    /// The class of the refusal.
    #[must_use]
    pub fn kind(&self) -> MemoryRefusalKind {
        self.kind
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
        let kind = match &e {
            WorkspaceError::Conflict { .. } => MemoryRefusalKind::Conflict,
            WorkspaceError::Cap(_) => MemoryRefusalKind::Cap,
            WorkspaceError::NulInContent { .. } => MemoryRefusalKind::Path,
            WorkspaceError::Sealed => MemoryRefusalKind::Sealed,
            WorkspaceError::Store(_) => MemoryRefusalKind::Store,
        };
        MemoryCommandError { text: e.to_string(), cause, kind }
    }
}

impl From<PathError> for MemoryCommandError {
    fn from(e: PathError) -> Self {
        MemoryCommandError::new(e.to_string())
    }
}

impl From<CapError> for MemoryCommandError {
    fn from(e: CapError) -> Self {
        MemoryCommandError {
            kind: MemoryRefusalKind::Cap,
            ..MemoryCommandError::new(e.to_string())
        }
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
        MemoryCommand::Delete { path } => delete(ws, &path).await,
        MemoryCommand::Rename { old_path, new_path } => rename(ws, &old_path, &new_path).await,
    }
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
    let body = number_lines_cut(&lines[start - 1..end], start);
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
    if let Some(ancestor) = file_ancestor(ws, &file).await? {
        return Err(MemoryCommandError::new(format!(
            "Error: Cannot create the file {tool}. The file {} exists and a file cannot be in a file.",
            ancestor.to_tool_path()
        )));
    }
    ws.write(&file, text).await?;
    Ok(format!("File created successfully at: {tool}"))
}

/// The first existing file that is a directory of the path, if there is one.
async fn file_ancestor(ws: &mut MemoryWorkspace, file: &MemoryPath) -> Result<Option<MemoryPath>, MemoryCommandError> {
    let mut prefix = String::new();
    let segments: Vec<&str> = file.as_str().split('/').skip(1).collect();
    for segment in &segments[..segments.len() - 1] {
        prefix.push('/');
        prefix.push_str(segment);
        let ancestor = MemoryPath::parse(&prefix)?;
        if ws.exists(&ancestor).await? {
            return Ok(Some(ancestor));
        }
    }
    Ok(None)
}

/// The files a path argument names: the file, or every file under the directory. An empty list
/// means that the path does not exist. A path that is a file and a directory is a clash.
async fn targets(ws: &mut MemoryWorkspace, parsed: &ParsedToolPath, raw: &str) -> Result<Vec<MemoryPath>, MemoryCommandError> {
    let under = ws.list(&parsed.dir).await?;
    let is_file = match &parsed.file {
        Some(f) => ws.exists(f).await?,
        None => false,
    };
    match (&parsed.file, is_file, under.is_empty() || parsed.dir.is_root()) {
        (Some(_), true, false) => Err(MemoryCommandError::new(format!(
            "Error: {} is both a file and a directory. Delete one of them first.",
            raw.strip_suffix('/').unwrap_or(raw)
        ))),
        (Some(f), true, true) => Ok(vec![f.clone()]),
        _ => Ok(under.into_iter().map(|(p, _)| p).collect()),
    }
}

fn missing_path(raw: &str) -> MemoryCommandError {
    MemoryCommandError::new(format!("Error: The path {} does not exist", raw.strip_suffix('/').unwrap_or(raw)))
}

async fn delete(ws: &mut MemoryWorkspace, raw: &str) -> Result<String, MemoryCommandError> {
    let parsed = parse_tool_path(raw)?;
    let shown = raw.strip_suffix('/').unwrap_or(raw);
    if parsed.dir.is_root() {
        return Err(MemoryCommandError::new(format!("Error: The memory root {shown} cannot be deleted.")));
    }
    let mut files = targets(ws, &parsed, raw).await?;
    if files.is_empty() {
        return Err(missing_path(raw));
    }
    let as_file = parsed.file.as_ref().is_some_and(|f| files.len() == 1 && files[0] == *f);
    let mut guards = Vec::new();
    if !as_file {
        // A directory delete checks the store listing first and then uses the checked rows. The
        // guard keeps the directory checked until the commit.
        ws.check_dir(&parsed.dir).await?;
        files = targets(ws, &parsed, raw).await?;
        if files.is_empty() {
            return Err(missing_path(raw));
        }
        guards.push(parsed.dir.clone());
    }
    ws.stage_guarded(Vec::new(), files, guards).await?;
    Ok(format!("Successfully deleted {shown}"))
}

async fn rename(ws: &mut MemoryWorkspace, old_raw: &str, new_raw: &str) -> Result<String, MemoryCommandError> {
    let old = parse_tool_path(old_raw)?;
    let new = parse_tool_path(new_raw)?;
    let old_shown = old_raw.strip_suffix('/').unwrap_or(old_raw);
    let new_shown = new_raw.strip_suffix('/').unwrap_or(new_raw);
    if old.dir.is_root() {
        return Err(MemoryCommandError::new(format!("Error: The memory root {old_shown} cannot be renamed.")));
    }
    let mut sources = targets(ws, &old, old_raw).await?;
    if sources.is_empty() {
        return Err(missing_path(old_raw));
    }
    // A single file that is not a directory moves as a file. Anything else moves as a directory.
    let as_file = old.file.as_ref().filter(|f| sources.len() == 1 && sources[0] == **f);
    if as_file.is_none() {
        // A directory move checks the store listing first and then uses the checked rows.
        ws.check_dir(&old.dir).await?;
        sources = targets(ws, &old, old_raw).await?;
        if sources.is_empty() {
            return Err(missing_path(old_raw));
        }
    }
    if old.dir.contains_dir(&new.dir) && old.dir != new.dir {
        return Err(MemoryCommandError::new(format!("Error: {new_shown} is inside {old_shown}.")));
    }
    // The destination check reads the store again, not the cached rows. A file under the
    // destination or a file at the destination path itself stops the move.
    let exists = new.dir.is_root() || !ws.list_fresh(&new.dir).await?.is_empty() || ws.exists(&MemoryPath::parse(new.dir.as_str())?).await?;
    if exists {
        return Err(MemoryCommandError::new(format!("Error: The destination {new_shown} already exists")));
    }
    // Check every destination before the first write.
    let mut moves = Vec::with_capacity(sources.len());
    for source in &sources {
        let target = match as_file {
            Some(_) => new
                .file
                .clone()
                .ok_or_else(|| MemoryCommandError::new(format!("Error: The path {new_raw} ends with / and is a directory path. Give the path of a file.")))?,
            None => source.rebase(&old.dir, &new.dir).map_err(|e| match e {
                PathError::Invalid { path, reason } => MemoryCommandError::new(format!(
                    "Error: The rename would make the path /memories{path}, which is not valid: {}. Nothing changed.",
                    tool_reason(reason)
                )),
                other => other.into(),
            })?,
        };
        if let Some(ancestor) = file_ancestor(ws, &target).await? {
            return Err(MemoryCommandError::new(format!(
                "Error: Cannot rename to {new_shown}. The file {} exists and a file cannot be in a file.",
                ancestor.to_tool_path()
            )));
        }
        moves.push((source.clone(), target));
    }
    let mut contents = Vec::with_capacity(moves.len());
    for (source, _) in &moves {
        contents.push(ws.read_for_move(source).await?.ok_or_else(|| missing_path(&source.to_tool_path()))?);
    }
    let puts = moves.iter().map(|(_, target)| target.clone()).zip(contents).collect();
    let removes = moves.into_iter().map(|(source, _)| source).collect();
    // A directory move keeps the source and the destination checked until the commit.
    let guards = if as_file.is_none() { vec![old.dir.clone(), new.dir.clone()] } else { Vec::new() };
    ws.stage_guarded(puts, removes, guards).await?;
    Ok(format!("Successfully renamed {old_shown} to {new_shown}"))
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
            // The hits are in order, so count only the line breaks between one hit and the next.
            let (mut line, mut from) = (0, 0);
            let lines: Vec<String> = many
                .iter()
                .map(|&at| {
                    line += content[from..at].matches('\n').count();
                    from = at;
                    (line + 1).to_string()
                })
                .collect();
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
    use crate::memory::conformance::fresh_scope;
    use crate::memory::fake::InMemoryMemoryStore;
    use crate::memory::{MemoryCaps, MemoryScope};
    use crate::ports::MemoryApplyError;

    async fn ws_with(files: &[(&str, &str)]) -> MemoryWorkspace {
        let mut ws = MemoryWorkspace::new(Arc::new(InMemoryMemoryStore::default()), fresh_scope(), MemoryCaps::default());
        for (p, c) in files {
            ws.write(&MemoryPath::parse(p).unwrap(), (*c).to_string()).await.unwrap();
        }
        ws
    }

    /// A store that holds the files already, and a workspace over it with an empty buffer.
    async fn committed_parts(files: &[(&str, &str)], caps: MemoryCaps) -> (Arc<InMemoryMemoryStore>, MemoryScope, MemoryWorkspace) {
        let store = Arc::new(InMemoryMemoryStore::default());
        let scope = fresh_scope();
        let mut seed = MemoryWorkspace::new(store.clone(), scope.clone(), MemoryCaps::default());
        for (p, c) in files {
            seed.write(&MemoryPath::parse(p).unwrap(), (*c).to_string()).await.unwrap();
        }
        seed.commit().await.unwrap();
        let ws = MemoryWorkspace::new(store.clone(), scope.clone(), caps);
        (store, scope, ws)
    }

    async fn committed_ws(files: &[(&str, &str)]) -> MemoryWorkspace {
        committed_parts(files, MemoryCaps::default()).await.2
    }

    /// Change a file in the store as another writer would.
    async fn other_writer(store: &Arc<InMemoryMemoryStore>, scope: &MemoryScope, path: &str, text: &str) {
        let mut w = MemoryWorkspace::new(store.clone(), scope.clone(), MemoryCaps::default());
        w.write(&MemoryPath::parse(path).unwrap(), text.to_string()).await.unwrap();
        w.commit().await.unwrap();
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
        assert!(out.ends_with("\n[The view is cut at 16,000 characters. Use view_range to read the other lines. A line longer than the limit is shown cut.]"));
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
        let mut hits = ws_with(&[("/h", "xx\nb\nx")]).await;
        assert_eq!(
            run(&mut hits, sr("/h", "x")).await.unwrap_err(),
            "No replacement was performed. Multiple occurrences of old_str `x` in lines: 1, 1, 3. Please ensure it is unique"
        );
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
    async fn delete_file_and_dir() {
        let mut ws = committed_ws(&[("/f", "f"), ("/a/x", "x"), ("/a/y/z", "z"), ("/ab/x", "k")]).await;
        assert_eq!(run(&mut ws, json!({"command":"delete","path":"/memories/f"})).await.unwrap(), "Successfully deleted /memories/f");
        assert_eq!(content(&mut ws, "/f").await, None);
        assert_eq!(run(&mut ws, json!({"command":"delete","path":"/memories/a/"})).await.unwrap(), "Successfully deleted /memories/a");
        assert_eq!(content(&mut ws, "/a/x").await, None);
        assert_eq!(content(&mut ws, "/a/y/z").await, None);
        assert_eq!(content(&mut ws, "/ab/x").await.as_deref(), Some("k"));
    }

    #[tokio::test]
    async fn delete_root_refused() {
        let mut ws = committed_ws(&[("/a", "a")]).await;
        for p in ["/memories", "/memories/"] {
            let err = run(&mut ws, json!({"command":"delete","path":p})).await.unwrap_err();
            assert_eq!(err, "Error: The memory root /memories cannot be deleted.");
        }
        assert_eq!(content(&mut ws, "/a").await.as_deref(), Some("a"));
        assert!(ws.take_changes().changes.is_empty());
    }

    #[tokio::test]
    async fn delete_missing() {
        let mut ws = committed_ws(&[("/a", "a")]).await;
        let err = run(&mut ws, json!({"command":"delete","path":"/memories/nope"})).await.unwrap_err();
        assert_eq!(err, "Error: The path /memories/nope does not exist");
        assert!(ws.take_changes().changes.is_empty());
    }

    #[tokio::test]
    async fn rename_file_and_dir() {
        let mut ws = committed_ws(&[("/f", "f"), ("/a/x", "x"), ("/a/y/z", "z"), ("/ab/x", "k")]).await;
        let r = |o: &str, n: &str| json!({"command":"rename","old_path":o,"new_path":n});
        assert_eq!(run(&mut ws, r("/memories/f", "/memories/g")).await.unwrap(), "Successfully renamed /memories/f to /memories/g");
        assert_eq!(content(&mut ws, "/f").await, None);
        assert_eq!(content(&mut ws, "/g").await.as_deref(), Some("f"));
        assert_eq!(run(&mut ws, r("/memories/a", "/memories/b")).await.unwrap(), "Successfully renamed /memories/a to /memories/b");
        assert_eq!(content(&mut ws, "/a/x").await, None);
        assert_eq!(content(&mut ws, "/a/y/z").await, None);
        assert_eq!(content(&mut ws, "/b/x").await.as_deref(), Some("x"));
        assert_eq!(content(&mut ws, "/b/y/z").await.as_deref(), Some("z"));
        assert_eq!(content(&mut ws, "/ab/x").await.as_deref(), Some("k"));
    }

    #[tokio::test]
    async fn rename_refusals() {
        let mut ws = committed_ws(&[("/a/x", "x"), ("/b", "b"), ("/a/1/2/3/4/5/6/7", "deep")]).await;
        let r = |o: &str, n: &str| json!({"command":"rename","old_path":o,"new_path":n});
        assert_eq!(run(&mut ws, r("/memories/nope", "/memories/c")).await.unwrap_err(), "Error: The path /memories/nope does not exist");
        assert_eq!(run(&mut ws, r("/memories/a", "/memories/b")).await.unwrap_err(), "Error: The destination /memories/b already exists");
        assert_eq!(run(&mut ws, r("/memories/a", "/memories/a/q")).await.unwrap_err(), "Error: /memories/a/q is inside /memories/a.");
        assert_eq!(run(&mut ws, r("/memories", "/memories/c")).await.unwrap_err(), "Error: The memory root /memories cannot be renamed.");
        assert_eq!(run(&mut ws, r("/memories/b", "/memories")).await.unwrap_err(), "Error: The destination /memories already exists");
        // The deep file would get 9 segments.
        let err = run(&mut ws, r("/memories/a", "/memories/q/r")).await.unwrap_err();
        assert!(err.starts_with("Error: The rename would make the path /memories/q/r/1/2/3/4/5/6/7, which is not valid: "), "{err}");
        assert!(err.ends_with(". Nothing changed."), "{err}");
        assert!(ws.take_changes().changes.is_empty());
    }

    #[tokio::test]
    async fn rename_into_file_dir_clash() {
        let mut ws = committed_ws(&[("/c.md", "c"), ("/a/x", "x")]).await;
        let err = run(&mut ws, json!({"command":"rename","old_path":"/memories/a","new_path":"/memories/c.md/a"})).await.unwrap_err();
        assert_eq!(err, "Error: Cannot rename to /memories/c.md/a. The file /memories/c.md exists and a file cannot be in a file.");
        assert_eq!(content(&mut ws, "/a/x").await.as_deref(), Some("x"));
        assert_eq!(content(&mut ws, "/c.md/a/x").await, None);
        assert!(ws.take_changes().changes.is_empty());
    }

    #[tokio::test]
    async fn rename_with_zero_net_change_passes_at_the_caps() {
        let caps = MemoryCaps {
            max_store_files: 2,
            max_store_bytes: 4,
            ..MemoryCaps::default()
        };
        let (_s, _c, mut ws) = committed_parts(&[("/a/x", "xx"), ("/a/y", "yy")], caps).await;
        run(&mut ws, json!({"command":"rename","old_path":"/memories/a/x","new_path":"/memories/a/z"})).await.unwrap();
        run(&mut ws, json!({"command":"rename","old_path":"/memories/a","new_path":"/memories/b"})).await.unwrap();
        assert_eq!(content(&mut ws, "/b/z").await.as_deref(), Some("xx"));
        assert_eq!(content(&mut ws, "/b/y").await.as_deref(), Some("yy"));
        assert_eq!(content(&mut ws, "/a/y").await, None);
    }

    #[tokio::test]
    async fn delete_of_a_listed_file_changed_outside_the_run_is_a_conflict() {
        let (store, scope, mut ws) = committed_parts(&[("/k", "v1")], MemoryCaps::default()).await;
        run(&mut ws, json!({"command":"view","path":"/memories"})).await.unwrap();
        other_writer(&store, &scope, "/k", "console").await;
        let err = run(&mut ws, json!({"command":"delete","path":"/memories/k"})).await.unwrap_err();
        assert_eq!(
            err,
            "The file /memories/k changed outside this run. Your changes to it in this run are gone. Use view to read it again."
        );
        ws.commit().await.unwrap();
        let mut fresh = MemoryWorkspace::new(store, scope, MemoryCaps::default());
        assert_eq!(content(&mut fresh, "/k").await.as_deref(), Some("console"));
    }

    #[tokio::test]
    async fn the_ancestor_check_keeps_the_first_sight_version() {
        let (store, scope, mut ws) = committed_parts(&[("/a", "v1")], MemoryCaps::default()).await;
        run(&mut ws, json!({"command":"view","path":"/memories"})).await.unwrap();
        other_writer(&store, &scope, "/a", "console").await;
        let err = run(&mut ws, json!({"command":"create","path":"/memories/a/b","file_text":"x"})).await.unwrap_err();
        assert!(err.contains("The file /memories/a exists"), "{err}");
        let err = run(&mut ws, json!({"command":"create","path":"/memories/a","file_text":"mine"})).await.unwrap_err();
        assert!(err.starts_with("The file /memories/a changed outside this run."), "{err}");
        ws.commit().await.unwrap();
        let mut fresh = MemoryWorkspace::new(store, scope, MemoryCaps::default());
        assert_eq!(content(&mut fresh, "/a").await.as_deref(), Some("console"));
    }

    #[tokio::test]
    async fn rename_and_delete_with_a_conflict_change_nothing() {
        let (store, scope, mut ws) = committed_parts(&[("/a/x", "x"), ("/a/y", "y"), ("/k", "k")], MemoryCaps::default()).await;
        run(&mut ws, json!({"command":"view","path":"/memories/a"})).await.unwrap();
        // A buffered write: the rename reads /a/y from the buffer, so only stage sees the conflict.
        run(&mut ws, json!({"command":"create","path":"/memories/a/y","file_text":"mine"})).await.unwrap();
        other_writer(&store, &scope, "/a/y", "changed").await;
        let err = run(&mut ws, json!({"command":"rename","old_path":"/memories/a","new_path":"/memories/b"})).await.unwrap_err();
        assert!(err.starts_with("The file /memories/a/y changed outside this run."), "{err}");
        // Read the cached files again, then repeat for a delete.
        run(&mut ws, json!({"command":"view","path":"/memories/a"})).await.unwrap();
        other_writer(&store, &scope, "/a/x", "changed").await;
        let err = run(&mut ws, json!({"command":"delete","path":"/memories/a"})).await.unwrap_err();
        assert!(err.starts_with("The file /memories/a/x changed outside this run."), "{err}");
        assert!(ws.take_changes().changes.is_empty());
        let mut fresh = MemoryWorkspace::new(store, scope, MemoryCaps::default());
        assert_eq!(content(&mut fresh, "/a/x").await.as_deref(), Some("changed"));
        assert_eq!(content(&mut fresh, "/a/y").await.as_deref(), Some("changed"));
        assert_eq!(content(&mut fresh, "/b/x").await, None);
    }

    #[tokio::test]
    async fn rename_of_a_listed_source_changed_outside_the_run_is_a_conflict() {
        let (store, scope, mut ws) = committed_parts(&[("/a/x", "v1")], MemoryCaps::default()).await;
        // The listing records /a/x at its first version, with no content.
        run(&mut ws, json!({"command":"view","path":"/memories"})).await.unwrap();
        other_writer(&store, &scope, "/a/x", "v2").await;
        let err = run(&mut ws, json!({"command":"rename","old_path":"/memories/a","new_path":"/memories/b"})).await.unwrap_err();
        assert_eq!(
            err,
            "The file /memories/a/x changed outside this run. Your changes to it in this run are gone. Use view to read it again."
        );
        assert!(ws.take_changes().changes.is_empty());
        ws.commit().await.unwrap();
        let mut fresh = MemoryWorkspace::new(store, scope, MemoryCaps::default());
        assert_eq!(content(&mut fresh, "/a/x").await.as_deref(), Some("v2"));
        assert_eq!(content(&mut fresh, "/b/x").await, None);
    }

    #[tokio::test]
    async fn a_conflict_after_a_rename_keeps_the_source() {
        let (store, scope, mut ws) = committed_parts(&[("/a", "original")], MemoryCaps::default()).await;
        run(&mut ws, json!({"command":"rename","old_path":"/memories/a","new_path":"/memories/b"})).await.unwrap();
        other_writer(&store, &scope, "/b", "other").await;
        let err = run(&mut ws, json!({"command":"create","path":"/memories/b","file_text":"mine"})).await.unwrap_err();
        assert!(err.contains("Your related changes to /memories/a are also gone."), "{err}");
        ws.commit().await.unwrap();
        let mut fresh = MemoryWorkspace::new(store, scope, MemoryCaps::default());
        assert_eq!(content(&mut fresh, "/a").await.as_deref(), Some("original"));
        assert_eq!(content(&mut fresh, "/b").await.as_deref(), Some("other"));
    }

    #[tokio::test]
    async fn existing_clash_view_and_delete() {
        let store = Arc::new(InMemoryMemoryStore::default());
        let scope = fresh_scope();
        let mut seed = MemoryWorkspace::new(store.clone(), scope.clone(), MemoryCaps::default());
        seed.write(&MemoryPath::parse("/n").unwrap(), "file".into()).await.unwrap();
        seed.write(&MemoryPath::parse("/n/a").unwrap(), "inner".into()).await.unwrap();
        seed.commit().await.unwrap();
        let mut ws = MemoryWorkspace::new(store, scope, MemoryCaps::default());
        let out = run(&mut ws, json!({"command":"view","path":"/memories/n"})).await.unwrap();
        assert_eq!(out, "Here's the content of /memories/n with line numbers:\n     1\tfile");
        let err = run(&mut ws, json!({"command":"delete","path":"/memories/n"})).await.unwrap_err();
        assert_eq!(err, "Error: /memories/n is both a file and a directory. Delete one of them first.");
    }

    #[tokio::test]
    async fn sealed_workspace_refuses_every_command() {
        let mut ws = ws_with(&[("/a", "a")]).await;
        ws.take_changes();
        for v in [json!({"command":"view","path":"/memories"}), json!({"command":"delete","path":"/memories/a"})] {
            assert_eq!(run(&mut ws, v).await.unwrap_err(), "The run is ending. Memory is read-only now.");
        }
    }

    #[tokio::test]
    async fn refusals_carry_a_kind() {
        let mut ws = ws_with(&[]).await;
        let bad = parse_command(json!({"command":"nope"})).unwrap_err();
        assert_eq!(bad.kind(), MemoryRefusalKind::Path);
        let outside = execute(&mut ws, parse_command(json!({"command":"view","path":"/a"})).unwrap()).await.unwrap_err();
        assert_eq!(outside.kind(), MemoryRefusalKind::Path);
        ws.take_changes();
        let sealed = execute(&mut ws, parse_command(json!({"command":"view","path":"/memories"})).unwrap()).await.unwrap_err();
        assert_eq!(sealed.kind(), MemoryRefusalKind::Sealed);
        assert_eq!(sealed.kind().as_str(), "sealed");
    }

    /// Delete a file in the store as another writer would.
    async fn other_delete(store: &Arc<InMemoryMemoryStore>, scope: &MemoryScope, path: &str) {
        let mut w = MemoryWorkspace::new(store.clone(), scope.clone(), MemoryCaps::default());
        w.remove(&MemoryPath::parse(path).unwrap()).await.unwrap();
        w.commit().await.unwrap();
    }

    /// A new workspace over the store, to read what the store holds.
    fn fresh_ws(store: &Arc<InMemoryMemoryStore>, scope: &MemoryScope) -> MemoryWorkspace {
        MemoryWorkspace::new(store.clone(), scope.clone(), MemoryCaps::default())
    }

    fn assert_conflict_at(result: Result<(), MemoryApplyError>, path: &str) {
        match result {
            Err(MemoryApplyError::Conflict { path: p }) => assert_eq!(p.as_str(), path),
            other => panic!("expected a conflict at {path}, got {other:?}"),
        }
    }

    fn dir(raw: &str) -> MemoryDir {
        MemoryDir::parse(raw).unwrap()
    }

    fn rename_cmd(old: &str, new: &str) -> serde_json::Value {
        json!({"command":"rename","old_path":old,"new_path":new})
    }

    #[tokio::test]
    async fn dir_delete_then_an_outsider_conflicts_at_commit() {
        let (store, scope, mut ws) = committed_parts(&[("/tasks/a.md", "a")], MemoryCaps::default()).await;
        run(&mut ws, json!({"command":"delete","path":"/memories/tasks"})).await.unwrap();
        other_writer(&store, &scope, "/tasks/b.md", "b").await;
        assert_conflict_at(ws.commit().await, "/tasks/b.md");
        let mut fresh = fresh_ws(&store, &scope);
        assert_eq!(content(&mut fresh, "/tasks/a.md").await.as_deref(), Some("a"));
        assert_eq!(content(&mut fresh, "/tasks/b.md").await.as_deref(), Some("b"));
    }

    #[tokio::test]
    async fn dir_rename_then_an_outsider_at_the_source_conflicts_at_commit() {
        let (store, scope, mut ws) = committed_parts(&[("/tasks/a.md", "a")], MemoryCaps::default()).await;
        run(&mut ws, rename_cmd("/memories/tasks", "/memories/done")).await.unwrap();
        other_writer(&store, &scope, "/tasks/b.md", "b").await;
        assert_conflict_at(ws.commit().await, "/tasks/b.md");
        let mut fresh = fresh_ws(&store, &scope);
        assert_eq!(content(&mut fresh, "/tasks/a.md").await.as_deref(), Some("a"));
        assert_eq!(content(&mut fresh, "/done/a.md").await, None);
    }

    #[tokio::test]
    async fn dir_rename_then_an_outsider_at_the_destination_conflicts_at_commit() {
        let (store, scope, mut ws) = committed_parts(&[("/tasks/a.md", "a")], MemoryCaps::default()).await;
        run(&mut ws, rename_cmd("/memories/tasks", "/memories/done")).await.unwrap();
        other_writer(&store, &scope, "/done/z.md", "z").await;
        assert_conflict_at(ws.commit().await, "/done/z.md");
        let mut fresh = fresh_ws(&store, &scope);
        assert_eq!(content(&mut fresh, "/tasks/a.md").await.as_deref(), Some("a"));
        assert_eq!(content(&mut fresh, "/done/a.md").await, None);
    }

    #[tokio::test]
    async fn a_create_after_a_dir_delete_joins_and_a_conflict_drops_the_guard() {
        let (store, scope, mut ws) = committed_parts(&[("/tasks/a.md", "a")], MemoryCaps::default()).await;
        run(&mut ws, json!({"command":"delete","path":"/memories/tasks"})).await.unwrap();
        run(&mut ws, json!({"command":"create","path":"/memories/tasks/a.md","file_text":"new"})).await.unwrap();
        // Control: the guard is armed before the conflict.
        assert_eq!(ws.armed_guards(), vec![dir("/tasks")]);
        other_writer(&store, &scope, "/tasks/a.md", "other").await;
        let err = run(&mut ws, json!({"command":"create","path":"/memories/tasks/a.md","file_text":"again"})).await.unwrap_err();
        assert!(err.starts_with("The file /memories/tasks/a.md changed outside this run."), "{err}");
        assert!(ws.armed_guards().is_empty());
        let set = ws.take_changes();
        assert!(set.changes.is_empty());
        assert!(set.guards.is_empty());
    }

    #[tokio::test]
    async fn a_create_after_a_dir_delete_where_the_file_is_deleted_drops_the_guard() {
        let (store, scope, mut ws) = committed_parts(&[("/tasks/a.md", "a")], MemoryCaps::default()).await;
        run(&mut ws, json!({"command":"delete","path":"/memories/tasks"})).await.unwrap();
        run(&mut ws, json!({"command":"create","path":"/memories/tasks/a.md","file_text":"new"})).await.unwrap();
        assert_eq!(ws.armed_guards(), vec![dir("/tasks")]);
        // The store no longer holds the path, so only the group of the path can drop the guard.
        other_delete(&store, &scope, "/tasks/a.md").await;
        let err = run(&mut ws, json!({"command":"create","path":"/memories/tasks/a.md","file_text":"again"})).await.unwrap_err();
        assert!(err.starts_with("The file /memories/tasks/a.md changed outside this run."), "{err}");
        assert!(ws.armed_guards().is_empty());
        let set = ws.take_changes();
        assert!(set.changes.is_empty());
        assert!(set.guards.is_empty());
    }

    #[tokio::test]
    async fn an_early_conflict_under_a_dir_delete_drops_the_delete() {
        let (store, scope, mut ws) = committed_parts(&[("/tasks/a.md", "a")], MemoryCaps::default()).await;
        run(&mut ws, json!({"command":"view","path":"/memories/tasks"})).await.unwrap();
        run(&mut ws, json!({"command":"delete","path":"/memories/tasks"})).await.unwrap();
        run(&mut ws, json!({"command":"create","path":"/memories/tasks/n.md","file_text":"mine"})).await.unwrap();
        other_writer(&store, &scope, "/tasks/n.md", "other").await;
        let err = run(&mut ws, json!({"command":"create","path":"/memories/tasks/n.md","file_text":"again"})).await.unwrap_err();
        assert!(err.starts_with("The file /memories/tasks/n.md changed outside this run."), "{err}");
        assert!(err.contains("Your related changes to /memories/tasks/a.md are also gone."), "{err}");
        assert!(ws.armed_guards().is_empty());
        ws.commit().await.unwrap();
        let mut fresh = fresh_ws(&store, &scope);
        assert_eq!(content(&mut fresh, "/tasks/a.md").await.as_deref(), Some("a"));
        assert_eq!(content(&mut fresh, "/tasks/n.md").await.as_deref(), Some("other"));
    }

    #[tokio::test]
    async fn a_guard_only_dir_delete_drops_on_an_early_conflict() {
        let (store, scope, mut ws) = committed_parts(&[], MemoryCaps::default()).await;
        run(&mut ws, json!({"command":"create","path":"/memories/t/x","file_text":"x"})).await.unwrap();
        run(&mut ws, json!({"command":"delete","path":"/memories/t"})).await.unwrap();
        // The delete cancels the buffered create, so only the guard is left.
        assert_eq!(ws.armed_guards(), vec![dir("/t")]);
        other_writer(&store, &scope, "/t/y", "y").await;
        let err = run(&mut ws, json!({"command":"create","path":"/memories/t/y","file_text":"mine"})).await.unwrap_err();
        assert!(err.starts_with("The file /memories/t/y changed outside this run."), "{err}");
        let set = ws.take_changes();
        assert!(set.guards.is_empty());
        assert!(set.changes.is_empty());
    }

    #[tokio::test]
    async fn dir_rename_and_delete_refuse_an_outsider_at_command_time() {
        let conflict = "The file /memories/tasks/b.md changed outside this run. Your changes to it in this run are gone. Use view to read it again.";
        for cmd in [rename_cmd("/memories/tasks", "/memories/done"), json!({"command":"delete","path":"/memories/tasks"})] {
            let (store, scope, mut ws) = committed_parts(&[("/tasks/a.md", "a")], MemoryCaps::default()).await;
            run(&mut ws, json!({"command":"view","path":"/memories/tasks"})).await.unwrap();
            other_writer(&store, &scope, "/tasks/b.md", "b").await;
            assert_eq!(run(&mut ws, cmd.clone()).await.unwrap_err(), conflict, "{cmd}");
            let set = ws.take_changes();
            assert!(set.changes.is_empty(), "{cmd}");
            assert!(set.guards.is_empty(), "{cmd}");
        }
    }

    #[tokio::test]
    async fn a_directory_view_after_a_conflict_lets_the_retry_pass() {
        let (store, scope, mut ws) = committed_parts(&[("/tasks/a.md", "a")], MemoryCaps::default()).await;
        run(&mut ws, json!({"command":"view","path":"/memories/tasks"})).await.unwrap();
        other_writer(&store, &scope, "/tasks/b.md", "b").await;
        let err = run(&mut ws, rename_cmd("/memories/tasks", "/memories/done")).await.unwrap_err();
        assert!(err.starts_with("The file /memories/tasks/b.md changed outside this run."), "{err}");
        let out = run(&mut ws, json!({"command":"view","path":"/memories/tasks"})).await.unwrap();
        assert!(out.contains("/memories/tasks/b.md"), "{out}");
        run(&mut ws, rename_cmd("/memories/tasks", "/memories/done")).await.unwrap();
        ws.commit().await.unwrap();
        let mut fresh = fresh_ws(&store, &scope);
        assert_eq!(content(&mut fresh, "/done/a.md").await.as_deref(), Some("a"));
        assert_eq!(content(&mut fresh, "/done/b.md").await.as_deref(), Some("b"));
        assert_eq!(content(&mut fresh, "/tasks/a.md").await, None);
        assert_eq!(content(&mut fresh, "/tasks/b.md").await, None);
    }

    #[tokio::test]
    async fn check_dir_catches_an_outsider_after_a_buffered_delete() {
        // `c.md` keeps the directory present after the buffered delete of `a.md`.
        let (store, scope, mut ws) = committed_parts(&[("/tasks/a.md", "a"), ("/tasks/c.md", "c")], MemoryCaps::default()).await;
        run(&mut ws, json!({"command":"view","path":"/memories/tasks"})).await.unwrap();
        run(&mut ws, json!({"command":"delete","path":"/memories/tasks/a.md"})).await.unwrap();
        other_writer(&store, &scope, "/tasks/b.md", "b").await;
        let err = run(&mut ws, rename_cmd("/memories/tasks", "/memories/done")).await.unwrap_err();
        assert!(err.starts_with("The file /memories/tasks/b.md changed outside this run."), "{err}");
        assert_eq!(content(&mut ws, "/done/c.md").await, None);
        assert_eq!(content(&mut ws, "/tasks/c.md").await.as_deref(), Some("c"));
    }

    #[tokio::test]
    async fn a_file_at_the_destination_gives_already_exists() {
        let exists = "Error: The destination /memories/done already exists";
        // A file under the destination, added after the run listed the destination as empty.
        let (store, scope, mut ws) = committed_parts(&[("/tasks/a.md", "a")], MemoryCaps::default()).await;
        let _ = run(&mut ws, json!({"command":"view","path":"/memories/done"})).await;
        other_writer(&store, &scope, "/done/z.md", "z").await;
        assert_eq!(run(&mut ws, rename_cmd("/memories/tasks", "/memories/done")).await.unwrap_err(), exists);
        let set = ws.take_changes();
        assert!(set.changes.is_empty());
        assert!(set.guards.is_empty());
        // A file at the path of the destination itself.
        let (store, scope, mut ws) = committed_parts(&[("/tasks/a.md", "a")], MemoryCaps::default()).await;
        other_writer(&store, &scope, "/done", "d").await;
        assert_eq!(run(&mut ws, rename_cmd("/memories/tasks", "/memories/done")).await.unwrap_err(), exists);
        let set = ws.take_changes();
        assert!(set.changes.is_empty());
        assert!(set.guards.is_empty());
    }

    #[tokio::test]
    async fn a_stale_cached_row_gives_the_dir_missing_text() {
        let (store, scope, mut ws) = committed_parts(&[], MemoryCaps::default()).await;
        run(&mut ws, json!({"command":"view","path":"/memories"})).await.unwrap();
        other_writer(&store, &scope, "/tasks/x", "x").await;
        run(&mut ws, json!({"command":"view","path":"/memories/tasks"})).await.unwrap();
        other_delete(&store, &scope, "/tasks/x").await;
        let err = run(&mut ws, rename_cmd("/memories/tasks", "/memories/done")).await.unwrap_err();
        assert_eq!(err, "Error: The path /memories/tasks does not exist");
    }

    #[tokio::test]
    async fn a_create_after_a_dir_delete_commits() {
        let (store, scope, mut ws) = committed_parts(&[("/tasks/a.md", "a")], MemoryCaps::default()).await;
        run(&mut ws, json!({"command":"delete","path":"/memories/tasks"})).await.unwrap();
        run(&mut ws, json!({"command":"create","path":"/memories/tasks/new.md","file_text":"n"})).await.unwrap();
        ws.commit().await.unwrap();
        let mut fresh = fresh_ws(&store, &scope);
        assert_eq!(content(&mut fresh, "/tasks/a.md").await, None);
        assert_eq!(content(&mut fresh, "/tasks/new.md").await.as_deref(), Some("n"));
    }

    #[tokio::test]
    async fn rename_there_and_back_commits() {
        let (store, scope, mut ws) = committed_parts(&[("/a", "A")], MemoryCaps::default()).await;
        run(&mut ws, rename_cmd("/memories/a", "/memories/b")).await.unwrap();
        run(&mut ws, rename_cmd("/memories/b", "/memories/a")).await.unwrap();
        ws.commit().await.unwrap();
        let mut fresh = fresh_ws(&store, &scope);
        assert_eq!(content(&mut fresh, "/a").await.as_deref(), Some("A"));
        assert_eq!(content(&mut fresh, "/b").await, None);
        let all = fresh.list(&MemoryDir::root()).await.unwrap();
        assert_eq!(all.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), ["/a"]);
    }

    #[tokio::test]
    async fn a_dir_rename_after_a_subdir_listing_moves_everything() {
        let (store, scope, mut ws) = committed_parts(&[("/tasks/a.md", "a"), ("/tasks/sub/b.md", "b")], MemoryCaps::default()).await;
        run(&mut ws, json!({"command":"view","path":"/memories/tasks/sub"})).await.unwrap();
        run(&mut ws, json!({"command":"view","path":"/memories/tasks"})).await.unwrap();
        run(&mut ws, rename_cmd("/memories/tasks", "/memories/done")).await.unwrap();
        let missing = "The path /memories/tasks/sub does not exist. Please provide a valid path.";
        assert_eq!(run(&mut ws, json!({"command":"view","path":"/memories/tasks/sub"})).await.unwrap_err(), missing);
        ws.commit().await.unwrap();
        let mut fresh = fresh_ws(&store, &scope);
        assert_eq!(run(&mut fresh, json!({"command":"view","path":"/memories/tasks/sub"})).await.unwrap_err(), missing);
        assert_eq!(content(&mut fresh, "/done/a.md").await.as_deref(), Some("a"));
        assert_eq!(content(&mut fresh, "/done/sub/b.md").await.as_deref(), Some("b"));
    }
}
