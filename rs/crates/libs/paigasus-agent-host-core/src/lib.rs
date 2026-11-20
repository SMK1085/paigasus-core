// SPDX-License-Identifier: Apache-2.0

//! Pure agent-host domain (M0 skeleton): typed IDs, value objects and port traits.
//! No I/O crate is a dependency of this crate (anti-goal A1). IDs and PRNs come from
//! `paigasus-kernel`.

pub mod agent;
pub mod deferral;
pub mod error;
pub mod ids;
pub mod inbox;
pub mod outbox;
pub mod run;
pub mod timer;

#[cfg(test)]
mod value_object_tests;

#[cfg(test)]
mod tests {
    const ALLOWED: &[&str] = &["paigasus-kernel", "uuid", "jiff", "thiserror", "async-trait", "serde", "serde_json"];

    /// The kind of a table header: `Some(None)` is a table of dependencies, `Some(Some(name))` is
    /// the table of one dependency, `None` is any other table (dev-dependencies included).
    fn dependency_table(header: &str) -> Option<Option<String>> {
        let h = header.trim().trim_start_matches('[').trim_end_matches(']').trim();
        for marker in ["dependencies", "build-dependencies"] {
            if h == marker || h.ends_with(&format!(".{marker}")) {
                return Some(None);
            }
            let dotted = format!(".{marker}.");
            if let Some(pos) = h.rfind(&dotted) {
                return Some(Some(h[pos + dotted.len()..].trim_matches(['"', '\'']).to_string()));
            }
            if let Some(name) = h.strip_prefix(&format!("{marker}.")) {
                return Some(Some(name.trim_matches(['"', '\'']).to_string()));
            }
        }
        None
    }

    /// Every dependency key of every non-dev dependency table in a manifest: `[dependencies]`,
    /// `[dependencies.<name>]`, `[build-dependencies]` and the `[target.<cfg>.…]` forms.
    fn dependency_keys(manifest: &str) -> Vec<String> {
        let mut keys = Vec::new();
        let mut table: Option<Option<String>> = None;
        for line in manifest.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                table = dependency_table(line);
                if let Some(Some(name)) = &table {
                    keys.push(name.clone());
                }
            } else if matches!(table, Some(None)) && !line.is_empty() && !line.starts_with('#') {
                let key = line.split('=').next().expect("split yields one item").trim();
                keys.push(key.split('.').next().expect("split yields one item").trim().to_string());
            }
        }
        keys
    }

    /// The keys that the A1 guard refuses.
    fn forbidden(manifest: &str) -> Vec<String> {
        dependency_keys(manifest).into_iter().filter(|k| !ALLOWED.contains(&k.as_str())).collect()
    }

    /// A1: the core crate depends on no I/O crate. The guard reads every non-dev dependency table
    /// of this crate's manifest. It asserts a subset: the dependencies arrive over several tasks,
    /// and `repo:machete` forbids a dependency with no use.
    #[test]
    fn the_core_crate_depends_on_no_io_crate() {
        const MANIFEST: &str = include_str!("../Cargo.toml");
        assert!(!dependency_keys(MANIFEST).is_empty(), "[dependencies] must not be empty");
        assert_eq!(forbidden(MANIFEST), Vec::<String>::new(), "a dependency is not allowed in the core crate (A1)");
    }

    #[test]
    fn the_a1_guard_reads_every_dependency_table() {
        let cases = [
            ("[dependencies.tokio]\nversion = \"1\"\n", "tokio"),
            ("[target.'cfg(unix)'.dependencies]\ntokio = \"1\"\n", "tokio"),
            ("[build-dependencies]\ntokio = \"1\"\n", "tokio"),
            ("[target.'cfg(unix)'.dependencies.tokio]\nversion = \"1\"\n", "tokio"),
            ("[dependencies]\ntokio.workspace = true\n", "tokio"),
        ];
        for (manifest, bad) in cases {
            assert_eq!(forbidden(manifest), vec![bad.to_string()], "{manifest}");
        }
        assert!(forbidden("[dependencies]\nuuid = \"1\"\n[dev-dependencies]\ntokio = \"1\"\n").is_empty());
    }
}
