// SPDX-License-Identifier: Apache-2.0
//! Split a Dogwood policy set into one unit for each policy, and lower all units at once.
//!
//! The set is lowered with ONE `lower_many` call that has one entry for each policy. The
//! distincter of an entry comes from the policy's `@id`, so a slot name does not change when
//! another policy is added or deleted (spec 2026-11-15 iam-tool-policy-sets, section 3.8, D1).

use dogwood_language::{Distincter, Error, LoweredPolicySet, ParsedPolicySet, PolicySchema, ServiceSchema, UNPINNED_EVENT_SCHEMA, lower_many};

/// One policy of a source set, re-rendered by Dogwood as its own `.dw` text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyUnit {
    /// The 0-based position of the policy in the source set.
    pub index: usize,
    /// The value of the `@id` annotation, if the policy has one.
    pub id: Option<String>,
    /// The value of the `@scope` annotation, if the policy has one.
    pub scope: Option<String>,
    /// Dogwood's macro-expanded rendering of the policy (`ParsedPolicy::expanded_source`).
    pub source: String,
    /// The annotations of the policy in source order. `None` is an annotation with no value.
    pub annotations: Vec<(String, Option<String>)>,
}

/// The service schema of the compiler: the unpinned event schema, the default macros and no
/// providers. History is partitioned by `@scope`, not by Dogwood pins.
pub fn service_schema() -> ServiceSchema {
    ServiceSchema::builder()
        .event_schema_str(UNPINNED_EVENT_SCHEMA)
        .build()
        .expect("Dogwood's built-in unpinned event schema parses")
}

/// Parse the whole source once, then make one unit for each policy.
pub fn split_policies(source: &str, svc: &ServiceSchema) -> Result<Vec<PolicyUnit>, Error> {
    let parsed = ParsedPolicySet::parse(source, svc)?;
    Ok(parsed
        .policies()
        .map(|policy| {
            let source = policy.expanded_source();
            let annotations = read_annotations(&source);
            let value_of = |key: &str| annotations.iter().find(|(k, _)| k == key).and_then(|(_, v)| v.clone());
            PolicyUnit {
                index: policy.index(),
                id: value_of("id"),
                scope: value_of("scope"),
                annotations: annotations.clone(),
                source,
            }
        })
        .collect())
}

/// Read the leading annotation lines of Dogwood's rendering of one policy.
///
/// Dogwood writes each annotation on its own line, as `@key` or `@key("value")`, before the
/// effect (`dogwood-language/src/render.rs`). The value uses Dogwood's `quote` escapes, and
/// this function decodes them.
pub fn read_annotations(expanded: &str) -> Vec<(String, Option<String>)> {
    expanded
        .lines()
        .map_while(|line| line.strip_prefix('@'))
        .map(|rest| match rest.split_once('(') {
            Some((key, value)) => {
                let quoted = value.strip_suffix(')').unwrap_or(value);
                (key.to_string(), Some(unquote(quoted)))
            }
            None => (rest.to_string(), None),
        })
        .collect()
}

/// Decode a string literal that Dogwood's `quote` wrote.
fn unquote(quoted: &str) -> String {
    let inner = quoted.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(quoted);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('0') => out.push('\0'),
            Some('u') => {
                let hex: String = chars.by_ref().skip_while(|&h| h == '{').take_while(|&h| h != '}').collect();
                match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    Some(decoded) => out.push(decoded),
                    None => {
                        out.push_str("\\u{");
                        out.push_str(&hex);
                        out.push('}');
                    }
                }
            }
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// The `lower_many` distincter for a policy `@id`: `"p_"` + the id, with `-` written as `_h` and
/// `_` written as `_u`. The mapping is injective.
///
/// The id must match the `@id` grammar `[a-z0-9_-]` (spec section 5.3). Every such id gives a
/// valid Cedar identifier, because the result starts with `p_`. The function panics on an id
/// with another character; the caller validates the id first.
pub fn distincter_for(id: &str) -> Distincter {
    let mut name = String::with_capacity(id.len() + 2);
    name.push_str("p_");
    for c in id.chars() {
        match c {
            '-' => name.push_str("_h"),
            '_' => name.push_str("_u"),
            c => name.push(c),
        }
    }
    name.parse().unwrap_or_else(|_| panic!("the @id {id:?} gives the invalid distincter {name:?}"))
}

/// Lower every unit in ONE `lower_many` call. Each unit is parsed again as its own set, and its
/// entry gets the distincter of its `@id`. A unit with no `@id` is an error.
pub fn lower_units(units: &[PolicyUnit], svc: &ServiceSchema, schema: &PolicySchema) -> Result<LoweredPolicySet, Error> {
    let parsed = units
        .iter()
        .map(|unit| {
            let id = unit.id.as_deref().ok_or_else(|| Error::InvalidDistincter(format!("policy {} has no @id", unit.index)))?;
            Ok((ParsedPolicySet::parse(&unit.source, svc)?, distincter_for(id)))
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let entries: Vec<(&ParsedPolicySet, Distincter)> = parsed.iter().map(|(set, distincter)| (set, distincter.clone())).collect();
    lower_many(&entries, schema)
}

/// The Cedar policy id of a policy, which is also the prefix of its slots. Each slot of the
/// policy is `<prefix>__temporal_<j>`.
pub fn slot_prefix(id: &str) -> String {
    format!("{}_0", distincter_for(id).as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_annotations_decodes_the_quote_escapes_and_stops_at_the_effect() {
        let rendered = "@id(\"a-b\")\n@flag\n@note(\"q\\\"b\\\\s\\nn\\u{1}\")\npermit (principal, action, resource);\n@late(\"x\")";
        assert_eq!(
            read_annotations(rendered),
            vec![
                ("id".to_string(), Some("a-b".to_string())),
                ("flag".to_string(), None),
                ("note".to_string(), Some("q\"b\\s\nn\u{1}".to_string())),
            ]
        );
    }
}
