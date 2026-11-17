// SPDX-License-Identifier: Apache-2.0

//! Tool import parsers and manual entry building (spec section 4.2, "Import").
//!
//! An import is all-or-nothing: any problem returns the full list of problems and no tools.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::json_schema::{MappingError, map_input_schema};
use super::model::{CatalogLimits, ToolEntry, ToolKind, ToolSource};
use super::name::parse_tool_name;

/// The three accepted import forms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportFormat {
    /// `[{"type":"function","function":{"name","description","parameters"}}]`
    OpenAiChatCompletions,
    /// `[{"type":"function","name",...}, {"type":"custom","name",...}]`
    OpenAiResponses,
    /// `{"tools":[{"name","inputSchema",...}]}`
    McpToolsList,
}

/// The result of a successful import.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedTools {
    pub tools: Vec<ToolEntry>,
    /// Entries of another type: the index in the document and the `type` value.
    pub skipped: Vec<(usize, String)>,
}

/// One reason why a batch cannot be accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogProblem {
    Invalid { name: Option<String>, message: String },
    Conflict { name_key: String },
}

fn invalid(name: Option<&str>, message: impl Into<String>) -> CatalogProblem {
    CatalogProblem::Invalid {
        name: name.map(|n| n.chars().take(64).collect()),
        message: message.into(),
    }
}

fn build_entry(namespace: Option<&str>, name: &str, kind: ToolKind, input_schema: Option<Value>, source: ToolSource, limits: &CatalogLimits) -> Result<ToolEntry, CatalogProblem> {
    let tool_name = parse_tool_name(namespace, name).map_err(|e| invalid(Some(name), e.to_string()))?;
    let shown = tool_name.display.clone();
    // A custom tool takes free text, so it keeps no schema and has an empty record.
    let input_schema = if kind == ToolKind::Custom { None } else { input_schema };
    let (cedar_input_type, dropped_fields) = match &input_schema {
        Some(schema) => {
            let bytes = serde_json::to_vec(schema).map_or(usize::MAX, |v| v.len());
            if bytes > limits.max_input_schema_bytes {
                return Err(invalid(Some(&shown), format!("the input schema is larger than {} bytes", limits.max_input_schema_bytes)));
            }
            map_input_schema(schema, limits.max_depth).map_err(|e| match e {
                MappingError::TooDeep => invalid(Some(&shown), format!("the input schema nests deeper than {} levels", limits.max_depth)),
                MappingError::NotAnObject => invalid(Some(&shown), "the input schema is not an object schema"),
            })?
        }
        None => (json!({ "type": "Record", "attributes": {} }), Vec::new()),
    };
    Ok(ToolEntry {
        name: tool_name,
        kind,
        input_schema,
        cedar_input_type,
        dropped_fields,
        source,
    })
}

/// Build one catalog entry from a manual request. `qualified` has at most one dot.
pub fn build_manual(qualified: &str, kind: ToolKind, input_schema: Option<Value>, limits: &CatalogLimits) -> Result<ToolEntry, Vec<CatalogProblem>> {
    let parsed = super::name::parse_qualified(qualified).map_err(|e| vec![invalid(Some(qualified), e.to_string())])?;
    let (ns, name) = match parsed.display.split_once('.') {
        Some((ns, name)) => (Some(ns), name),
        None => (None, parsed.display.as_str()),
    };
    build_entry(ns, name, kind, input_schema, ToolSource::Manual, limits).map_err(|p| vec![p])
}

/// Parse an import document. Entries of another type are skipped and listed.
pub fn parse_import(format: ImportFormat, document: &Value, namespace: Option<&str>, limits: &CatalogLimits) -> Result<ImportedTools, Vec<CatalogProblem>> {
    let list = match (format, document) {
        (ImportFormat::McpToolsList, Value::Object(o)) => o.get("tools").and_then(Value::as_array),
        (ImportFormat::McpToolsList, _) => None,
        (_, Value::Array(a)) => Some(a),
        _ => None,
    };
    let Some(list) = list else {
        let expected = match format {
            ImportFormat::McpToolsList => "an object with a \"tools\" array",
            _ => "an array",
        };
        return Err(vec![invalid(None, format!("the document must be {expected}"))]);
    };
    if list.len() > limits.max_tools {
        return Err(vec![invalid(None, format!("the batch has more than {} tools", limits.max_tools))]);
    }

    let mut tools = Vec::new();
    let mut skipped = Vec::new();
    let mut problems = Vec::new();
    for (index, item) in list.iter().enumerate() {
        match parse_item(format, item, namespace, limits) {
            Ok(Parsed::Tool(entry)) => tools.push(*entry),
            Ok(Parsed::Skip(ty)) => skipped.push((index, ty)),
            Err(p) => problems.push(p),
        }
    }

    let mut seen = BTreeSet::new();
    let mut reported = BTreeSet::new();
    for t in &tools {
        if !seen.insert(t.name.name_key.clone()) && reported.insert(t.name.name_key.clone()) {
            problems.push(CatalogProblem::Conflict { name_key: t.name.name_key.clone() });
        }
    }
    if problems.is_empty() { Ok(ImportedTools { tools, skipped }) } else { Err(problems) }
}

enum Parsed {
    Tool(Box<ToolEntry>),
    Skip(String),
}

fn parse_item(format: ImportFormat, item: &Value, namespace: Option<&str>, limits: &CatalogLimits) -> Result<Parsed, CatalogProblem> {
    let tool = |name: &str, kind, schema: Option<&Value>, source| build_entry(namespace, name, kind, schema.filter(|v| !v.is_null()).cloned(), source, limits).map(|e| Parsed::Tool(Box::new(e)));
    let name_of = |v: &Value| v.get("name").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| invalid(None, "the tool has no name"));
    match format {
        ImportFormat::McpToolsList => {
            let name = name_of(item)?;
            tool(&name, ToolKind::Function, item.get("inputSchema"), ToolSource::Mcp)
        }
        ImportFormat::OpenAiChatCompletions => match type_of(item)? {
            "function" => {
                let f = item.get("function").ok_or_else(|| invalid(None, "the function tool has no \"function\" object"))?;
                let name = name_of(f)?;
                tool(&name, ToolKind::Function, f.get("parameters"), ToolSource::OpenAi)
            }
            other => Ok(Parsed::Skip(other.to_owned())),
        },
        ImportFormat::OpenAiResponses => match type_of(item)? {
            "function" => {
                let name = name_of(item)?;
                tool(&name, ToolKind::Function, item.get("parameters"), ToolSource::OpenAi)
            }
            "custom" => {
                let name = name_of(item)?;
                tool(&name, ToolKind::Custom, None, ToolSource::OpenAi)
            }
            other => Ok(Parsed::Skip(other.to_owned())),
        },
    }
}

fn type_of(item: &Value) -> Result<&str, CatalogProblem> {
    item.get("type").and_then(Value::as_str).ok_or_else(|| invalid(None, "the tool has no \"type\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> CatalogLimits {
        CatalogLimits::default()
    }

    fn params() -> Value {
        json!({"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"]})
    }

    #[test]
    fn openai_chat_completions_form_imports() {
        let doc = json!([{"type": "function", "function": {"name": "Search", "description": "d", "parameters": params()}}]);
        let out = parse_import(ImportFormat::OpenAiChatCompletions, &doc, None, &limits()).unwrap();
        assert_eq!(out.tools.len(), 1);
        let t = &out.tools[0];
        assert_eq!((t.name.name_key.as_str(), t.name.display.as_str()), ("search", "Search"));
        assert_eq!(t.kind, ToolKind::Function);
        assert_eq!(t.source, ToolSource::OpenAi);
        assert_eq!(t.input_schema, Some(params()));
        assert_eq!(t.cedar_input_type["attributes"]["q"], json!({"type": "String", "required": true}));
    }

    #[test]
    fn openai_responses_form_imports_function_and_custom() {
        let doc = json!([
            {"type": "function", "name": "fetch", "parameters": params()},
            {"type": "custom", "name": "apply_patch", "description": "free text"}
        ]);
        let out = parse_import(ImportFormat::OpenAiResponses, &doc, None, &limits()).unwrap();
        assert_eq!(out.tools[0].kind, ToolKind::Function);
        assert_eq!(out.tools[1].kind, ToolKind::Custom);
        assert_eq!(out.tools[1].input_schema, None);
        assert_eq!(out.tools[1].cedar_input_type, json!({"type": "Record", "attributes": {}}));
    }

    #[test]
    fn mcp_tools_list_imports_with_a_namespace() {
        let doc = json!({"tools": [{"name": "search", "inputSchema": params()}]});
        let out = parse_import(ImportFormat::McpToolsList, &doc, Some("gh"), &limits()).unwrap();
        assert_eq!(out.tools[0].name.name_key, "gh.search");
        assert_eq!(out.tools[0].source, ToolSource::Mcp);
    }

    #[test]
    fn other_types_are_skipped_and_listed() {
        let doc = json!([
            {"type": "function", "name": "a"},
            {"type": "web_search"},
            {"type": "file_search"}
        ]);
        let out = parse_import(ImportFormat::OpenAiResponses, &doc, None, &limits()).unwrap();
        assert_eq!(out.tools.len(), 1);
        assert_eq!(out.skipped, vec![(1, "web_search".to_owned()), (2, "file_search".to_owned())]);
    }

    #[test]
    fn an_mcp_name_outside_the_grammar_is_rejected_with_the_reason() {
        let doc = json!({"tools": [{"name": "bad name", "inputSchema": params()}]});
        let problems = parse_import(ImportFormat::McpToolsList, &doc, None, &limits()).unwrap_err();
        assert_eq!(problems.len(), 1);
        match &problems[0] {
            CatalogProblem::Invalid { name, message } => {
                assert_eq!(name.as_deref(), Some("bad name"));
                assert!(message.contains("bad name"), "{message}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn import_batch_with_two_case_variants_conflicts() {
        let doc = json!([{"type": "function", "name": "Search"}, {"type": "function", "name": "search"}]);
        let problems = parse_import(ImportFormat::OpenAiResponses, &doc, None, &limits()).unwrap_err();
        assert_eq!(problems, vec![CatalogProblem::Conflict { name_key: "search".into() }]);
    }

    #[test]
    fn an_input_schema_over_64_kib_is_invalid() {
        let big = json!({"type": "object", "properties": {"q": {"type": "string", "description": "x".repeat(70_000)}}});
        let doc = json!([{"type": "function", "name": "big", "parameters": big}]);
        let problems = parse_import(ImportFormat::OpenAiResponses, &doc, None, &limits()).unwrap_err();
        assert!(matches!(&problems[0], CatalogProblem::Invalid { name: Some(n), .. } if n == "big"));
    }

    #[test]
    fn a_schema_over_the_depth_limit_is_invalid() {
        let mut s = json!({"type": "object", "properties": {}});
        for _ in 0..9 {
            s = json!({"type": "object", "properties": {"n": s}});
        }
        let err = build_manual("deep", ToolKind::Function, Some(s), &limits()).unwrap_err();
        assert!(matches!(&err[0], CatalogProblem::Invalid { .. }));
    }

    #[test]
    fn an_input_schema_of_exactly_64_kib_is_allowed() {
        let make = |pad: usize| json!({"type": "object", "properties": {"q": {"type": "string", "description": "x".repeat(pad)}}});
        let base = serde_json::to_vec(&make(0)).unwrap().len();
        let exact = make(65536 - base);
        assert_eq!(serde_json::to_vec(&exact).unwrap().len(), 65536);
        assert!(build_manual("edge", ToolKind::Function, Some(exact), &limits()).is_ok());
        assert!(build_manual("edge", ToolKind::Function, Some(make(65537 - base)), &limits()).is_err());
    }

    #[test]
    fn a_very_long_name_is_capped_in_the_problem() {
        let long = "x".repeat(10_000);
        let doc = json!({"tools": [{"name": long}]});
        let problems = parse_import(ImportFormat::McpToolsList, &doc, None, &limits()).unwrap_err();
        match &problems[0] {
            CatalogProblem::Invalid { name, message } => {
                assert_eq!(name.as_ref().map(String::len), Some(64));
                assert!(message.len() < 200, "{}", message.len());
            }
            other => panic!("unexpected {other:?}"),
        }
        let manual = build_manual(&"y".repeat(10_000), ToolKind::Function, None, &limits()).unwrap_err();
        assert!(matches!(&manual[0], CatalogProblem::Invalid { name: Some(n), .. } if n.len() == 64));
    }

    #[test]
    fn a_null_parameters_value_means_no_schema() {
        let responses = json!([{"type": "function", "name": "a", "parameters": null}]);
        let chat = json!([{"type": "function", "function": {"name": "a", "parameters": null}}]);
        for (format, doc) in [(ImportFormat::OpenAiResponses, responses), (ImportFormat::OpenAiChatCompletions, chat)] {
            let out = parse_import(format, &doc, None, &limits()).unwrap();
            assert_eq!(out.tools[0].input_schema, None);
            assert_eq!(out.tools[0].cedar_input_type, json!({"type": "Record", "attributes": {}}));
        }
    }

    #[test]
    fn more_than_500_tools_is_invalid() {
        let tools: Vec<Value> = (0..501).map(|i| json!({"type": "function", "name": format!("t{i}")})).collect();
        let problems = parse_import(ImportFormat::OpenAiResponses, &Value::Array(tools.clone()), None, &limits()).unwrap_err();
        assert!(matches!(&problems[0], CatalogProblem::Invalid { name: None, .. }));
        assert!(parse_import(ImportFormat::OpenAiResponses, &Value::Array(tools[..500].to_vec()), None, &limits()).is_ok());
    }

    #[test]
    fn a_document_of_the_wrong_shape_is_invalid() {
        assert!(parse_import(ImportFormat::McpToolsList, &json!([]), None, &limits()).is_err());
        assert!(parse_import(ImportFormat::OpenAiChatCompletions, &json!({}), None, &limits()).is_err());
    }

    #[test]
    fn a_reserved_name_is_invalid() {
        let doc = json!([{"type": "function", "name": "InvokeTool"}]);
        assert!(parse_import(ImportFormat::OpenAiResponses, &doc, None, &limits()).is_err());
    }

    #[test]
    fn build_manual_keeps_the_given_case_and_a_custom_tool_has_no_schema() {
        let e = build_manual("Shell", ToolKind::Custom, Some(params()), &limits()).unwrap();
        assert_eq!((e.name.display.as_str(), e.name.name_key.as_str()), ("Shell", "shell"));
        assert_eq!(e.source, ToolSource::Manual);
        assert_eq!(e.input_schema, None);
        let ns = build_manual("gh.Search", ToolKind::Function, None, &limits()).unwrap();
        assert_eq!(ns.name.name_key, "gh.search");
        assert_eq!(ns.cedar_input_type, json!({"type": "Record", "attributes": {}}));
    }
}
