// SPDX-License-Identifier: Apache-2.0

//! The file policy source (SMA-752 spec 6.3.2). It reads this layout:
//!
//! ```text
//! <dir>/floor.cedar
//! <dir>/tenants/<org uuid>/policies.cedar
//! <dir>/tenants/<org uuid>/catalog.json   # ["read_file", "send_email", ...]
//! ```

use std::future::Future;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::domain::tool_policy::{PolicySource, PolicyText, SourceError, TenantPolicies};

const FLOOR_FILE: &str = "floor.cedar";
const TENANTS_DIR: &str = "tenants";
const POLICIES_FILE: &str = "policies.cedar";
const CATALOG_FILE: &str = "catalog.json";

/// Reads policies from a directory.
#[derive(Debug, Clone)]
pub struct FilePolicySource {
    dir: PathBuf,
}

impl FilePolicySource {
    /// A source that reads the directory `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The tenant read. `between` runs after the directory is resolved and before the first file
    /// is read. A test uses it to swap a symlink at that point.
    async fn tenant_inner<F: Future<Output = ()>>(&self, org: Uuid, between: F) -> Result<TenantPolicies, SourceError> {
        let link = self.dir.join(TENANTS_DIR).join(org.hyphenated().to_string());
        // Resolve the real path once. Both files are read under it (a ConfigMap symlink swap).
        let real = match tokio::fs::canonicalize(&link).await {
            Ok(path) => path,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                return Ok(TenantPolicies {
                    text: String::new(),
                    catalog: Vec::new(),
                    version: version_of(&[b"", b""]),
                });
            }
            Err(err) => return Err(io_error("tenant directory", &err)),
        };
        between.await;
        let policies = read_optional(&real.join(POLICIES_FILE), "tenant policies").await?;
        let catalog_raw = read_optional(&real.join(CATALOG_FILE), "tenant catalog").await?;
        let catalog = if catalog_raw.trim().is_empty() {
            Vec::new()
        } else {
            serde_json::from_str::<Vec<String>>(&catalog_raw).map_err(|_| SourceError::Malformed("tenant catalog is not an array of strings".to_owned()))?
        };
        let version = version_of(&[policies.as_bytes(), catalog_raw.as_bytes()]);
        Ok(TenantPolicies { text: policies, catalog, version })
    }
}

#[async_trait]
impl PolicySource for FilePolicySource {
    async fn floor(&self) -> Result<PolicyText, SourceError> {
        let path = self.dir.join(FLOOR_FILE);
        match tokio::fs::read_to_string(&path).await {
            Ok(text) => {
                let version = version_of(&[text.as_bytes()]);
                Ok(PolicyText { text, version })
            }
            Err(err) if err.kind() == ErrorKind::NotFound => Err(SourceError::NotFound("floor file".to_owned())),
            Err(err) => Err(io_error("floor file", &err)),
        }
    }

    async fn tenant(&self, org: Uuid) -> Result<TenantPolicies, SourceError> {
        self.tenant_inner(org, async {}).await
    }
}

/// A file that may be absent. An absent file reads as an empty string.
async fn read_optional(path: &Path, what: &str) -> Result<String, SourceError> {
    match tokio::fs::read_to_string(path).await {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(io_error(what, &err)),
    }
}

/// The message names the item and the error kind. It holds no path and no file content.
fn io_error(what: &str, err: &std::io::Error) -> SourceError {
    SourceError::Io(format!("{what}: {:?}", err.kind()))
}

/// The hex SHA-256 of the parts, with a zero byte between them.
fn version_of(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            hasher.update([0u8]);
        }
        hasher.update(part);
    }
    hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORG: Uuid = Uuid::from_u128(0x0190a100_0000_7000_8000_0000000000a1);

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn tenant_dir(root: &Path) -> PathBuf {
        root.join(TENANTS_DIR).join(ORG.hyphenated().to_string())
    }

    #[tokio::test]
    async fn a_missing_floor_is_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let src = FilePolicySource::new(dir.path());
        assert!(matches!(src.floor().await, Err(SourceError::NotFound(_))));
    }

    #[tokio::test]
    async fn an_empty_floor_is_an_empty_floor() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join(FLOOR_FILE), "");
        let floor = FilePolicySource::new(dir.path()).floor().await.unwrap();
        assert_eq!(floor.text, "");
        assert_eq!(floor.version.len(), 64);
    }

    #[tokio::test]
    async fn a_tenant_without_a_directory_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let tenant = FilePolicySource::new(dir.path()).tenant(ORG).await.unwrap();
        assert_eq!(tenant.text, "");
        assert!(tenant.catalog.is_empty());
    }

    #[tokio::test]
    async fn a_tenant_reads_its_policies_and_catalog() {
        let dir = tempfile::tempdir().unwrap();
        write(&tenant_dir(dir.path()).join(POLICIES_FILE), "// p");
        write(&tenant_dir(dir.path()).join(CATALOG_FILE), r#"["read_file","send_email"]"#);
        let tenant = FilePolicySource::new(dir.path()).tenant(ORG).await.unwrap();
        assert_eq!(tenant.text, "// p");
        assert_eq!(tenant.catalog, vec!["read_file".to_owned(), "send_email".to_owned()]);
    }

    #[tokio::test]
    async fn a_tenant_directory_without_files_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tenant_dir(dir.path())).unwrap();
        let tenant = FilePolicySource::new(dir.path()).tenant(ORG).await.unwrap();
        assert_eq!((tenant.text.as_str(), tenant.catalog.len()), ("", 0));
    }

    #[tokio::test]
    async fn a_bad_catalog_is_malformed() {
        for bad in [r#"{"a":1}"#, r#"["a", 1]"#, "not json"] {
            let dir = tempfile::tempdir().unwrap();
            write(&tenant_dir(dir.path()).join(CATALOG_FILE), bad);
            let err = FilePolicySource::new(dir.path()).tenant(ORG).await.unwrap_err();
            assert!(matches!(err, SourceError::Malformed(_)), "{bad}");
            assert!(!err.to_string().contains("not json"));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn both_tenant_files_come_from_one_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("v1").join(POLICIES_FILE), "// v1");
        write(&root.join("v1").join(CATALOG_FILE), r#"["one"]"#);
        write(&root.join("v2").join(POLICIES_FILE), "// v2");
        write(&root.join("v2").join(CATALOG_FILE), r#"["two"]"#);
        std::fs::create_dir_all(root.join(TENANTS_DIR)).unwrap();
        let link = tenant_dir(root);
        std::os::unix::fs::symlink(root.join("v1"), &link).unwrap();

        let src = FilePolicySource::new(root);
        // Swap the link to v2 after the directory is resolved and before the first read.
        let swap = async {
            std::fs::remove_file(&link).unwrap();
            std::os::unix::fs::symlink(root.join("v2"), &link).unwrap();
        };
        let tenant = src.tenant_inner(ORG, swap).await.unwrap();
        assert_eq!((tenant.text.as_str(), tenant.catalog.as_slice()), ("// v1", &["one".to_owned()][..]));
        // A later read sees v2 whole.
        let later = src.tenant(ORG).await.unwrap();
        assert_eq!((later.text.as_str(), later.catalog.as_slice()), ("// v2", &["two".to_owned()][..]));
    }

    #[tokio::test]
    async fn the_version_changes_with_the_content() {
        let dir = tempfile::tempdir().unwrap();
        let src = FilePolicySource::new(dir.path());
        write(&dir.path().join(FLOOR_FILE), "// a");
        let f1 = src.floor().await.unwrap().version;
        assert_eq!(f1, src.floor().await.unwrap().version);
        write(&dir.path().join(FLOOR_FILE), "// b");
        assert_ne!(f1, src.floor().await.unwrap().version);

        write(&tenant_dir(dir.path()).join(POLICIES_FILE), "// a");
        let t1 = src.tenant(ORG).await.unwrap().version;
        write(&tenant_dir(dir.path()).join(CATALOG_FILE), r#"["x"]"#);
        let t2 = src.tenant(ORG).await.unwrap().version;
        assert_ne!(t1, t2);
        // The separator keeps the boundary: moving a byte between the files changes the version.
        assert_ne!(version_of(&[b"ab", b"c"]), version_of(&[b"a", b"bc"]));
    }

    #[test]
    fn the_shipped_floor_compiles_and_holds_one_policy() {
        let text = include_str!("../../../tool-policies/floor.cedar");
        assert!(text.starts_with("// SPDX-License-Identifier: Apache-2.0\n"));
        assert!(text.contains(r#"@id("floor-name-collision")"#));
        let floor = paigasus_kernel::tool_policy::ToolPolicies::compile_floor(text).unwrap();
        assert_eq!(floor.len(), 1);
    }
}
