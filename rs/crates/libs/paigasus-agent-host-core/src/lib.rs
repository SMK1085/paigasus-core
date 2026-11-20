// SPDX-License-Identifier: Apache-2.0

//! Pure agent-host domain (M0 skeleton): typed IDs, value objects and port traits.
//! No I/O crate is a dependency of this crate (anti-goal A1). IDs and PRNs come from
//! `paigasus-kernel`.

pub mod error;
pub mod ids;

#[cfg(test)]
mod tests {
    /// A1: the core crate depends on no I/O crate. The guard reads the `[dependencies]` table of
    /// this crate's manifest. It asserts a subset: the dependencies arrive over several tasks, and
    /// `repo:machete` forbids a dependency with no use.
    #[test]
    fn the_core_crate_depends_on_no_io_crate() {
        const MANIFEST: &str = include_str!("../Cargo.toml");
        const ALLOWED: &[&str] = &["paigasus-kernel", "uuid", "jiff", "thiserror", "async-trait", "serde", "serde_json"];
        let mut in_deps = false;
        let mut keys = Vec::new();
        for line in MANIFEST.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_deps = line == "[dependencies]";
                continue;
            }
            if in_deps && !line.is_empty() && !line.starts_with('#') {
                let key = line.split('=').next().expect("split yields one item").trim();
                keys.push(key.to_string());
            }
        }
        assert!(!keys.is_empty(), "[dependencies] must not be empty");
        for key in &keys {
            assert!(ALLOWED.contains(&key.as_str()), "dependency `{key}` is not allowed in the core crate (A1)");
        }
    }
}
