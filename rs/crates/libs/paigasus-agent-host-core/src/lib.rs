// SPDX-License-Identifier: Apache-2.0

//! Pure agent-host domain (M0 skeleton): typed IDs, value objects and port traits.
//! No I/O crate is a dependency of this crate (anti-goal A1). IDs and PRNs come from
//! `paigasus-kernel`.

pub mod agent;
pub mod claim;
pub mod deferral;
pub mod error;
pub mod fault;
pub mod ids;
pub mod inbox;
pub mod lease;
pub mod memory;
pub mod mid_run;
pub mod outbox;
pub mod ports;
pub mod render;
pub mod run;
pub mod screen;
pub mod timer;

#[cfg(test)]
mod value_object_tests;

#[cfg(test)]
mod port_tests {
    use crate::ports::*;

    #[test]
    fn every_port_is_object_safe_send_and_sync() {
        fn assert_port<T: ?Sized + Send + Sync>() {}
        assert_port::<dyn InboxRepository>();
        assert_port::<dyn LeaseRepository>();
        assert_port::<dyn ClaimRepository>();
        assert_port::<dyn TimerRepository>();
        assert_port::<dyn Scheduler>();
        assert_port::<dyn TzResolver>();
        assert_port::<dyn RunClock>();
        assert_port::<dyn IdSource>();
        assert_port::<dyn MemoryStore>();
        assert_port::<dyn Outbox>();
        assert_port::<dyn AgentResolver>();
        assert_port::<dyn RunCommitter>();
        assert_port::<dyn EventScreen>();
        assert_port::<dyn FaultPoint>();
    }
}

#[cfg(test)]
mod tests {
    const ALLOWED: &[&str] = &["paigasus-kernel", "uuid", "jiff", "thiserror", "async-trait", "serde", "serde_json", "croner"];

    /// The kind of a table header: `Some(None)` is a table of dependencies, `Some(Some(name))` is
    /// the table of one dependency, `None` is any other table (dev-dependencies included).
    fn dependency_table(header: &str) -> Option<Option<String>> {
        // Drop a trailing TOML comment: the first `#` outside a quoted string.
        let mut quote: Option<char> = None;
        let mut end = header.len();
        for (i, c) in header.char_indices() {
            match (quote, c) {
                (None, '"' | '\'') => quote = Some(c),
                (Some(q), _) if c == q => quote = None,
                (None, '#') => {
                    end = i;
                    break;
                }
                _ => {}
            }
        }
        let h = header[..end].trim().trim_start_matches('[').trim_end_matches(']').trim();
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
    /// of this crate's manifest. It asserts set equality with the allowed set (spec §6.2).
    #[test]
    fn the_core_crate_depends_on_no_io_crate() {
        const MANIFEST: &str = include_str!("../Cargo.toml");
        assert!(!dependency_keys(MANIFEST).is_empty(), "[dependencies] must not be empty");
        assert_eq!(forbidden(MANIFEST), Vec::<String>::new(), "a dependency is not allowed in the core crate (A1)");
        let found: std::collections::BTreeSet<String> = dependency_keys(MANIFEST).into_iter().collect();
        let allowed: std::collections::BTreeSet<String> = ALLOWED.iter().map(|k| k.to_string()).collect();
        assert_eq!(found, allowed, "the dependency set must equal the spec section 6.2 set");
    }

    #[test]
    fn the_a1_guard_reads_every_dependency_table() {
        let cases = [
            ("[dependencies.tokio]\nversion = \"1\"\n", "tokio"),
            ("[target.'cfg(unix)'.dependencies]\ntokio = \"1\"\n", "tokio"),
            ("[build-dependencies]\ntokio = \"1\"\n", "tokio"),
            ("[target.'cfg(unix)'.dependencies.tokio]\nversion = \"1\"\n", "tokio"),
            ("[dependencies]\ntokio.workspace = true\n", "tokio"),
            ("[target.'cfg(unix)'.dependencies] # platform-only\ntokio = \"1\"\n", "tokio"),
            ("[dependencies.tokio] # [1]\nversion = \"1\"\n", "tokio"),
        ];
        for (manifest, bad) in cases {
            assert_eq!(forbidden(manifest), vec![bad.to_string()], "{manifest}");
        }
        assert!(forbidden("[dependencies]\nuuid = \"1\"\n[dev-dependencies]\ntokio = \"1\"\n").is_empty());
        assert!(forbidden("[dev-dependencies] # test-only\ntokio = \"1\"\n").is_empty());
    }
}
