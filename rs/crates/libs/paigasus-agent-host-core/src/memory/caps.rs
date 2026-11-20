// SPDX-License-Identifier: Apache-2.0

//! The caps of a memory store, and the one rule that checks them.

use super::MemoryPath;
use crate::ports::MemoryUsage;

/// The limits of a memory store. They are host config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryCaps {
    /// The largest file, in UTF-8 bytes.
    pub max_file_bytes: u64,
    /// The largest total of a scope, in UTF-8 bytes.
    pub max_store_bytes: u64,
    /// The largest number of files in a scope.
    pub max_store_files: u64,
    /// The largest file that `view` returns in full, in UTF-8 bytes.
    pub max_loaded_file_bytes: u64,
}

impl Default for MemoryCaps {
    fn default() -> Self {
        MemoryCaps {
            max_file_bytes: 65_536,
            max_store_bytes: 1_048_576,
            max_store_files: 512,
            max_loaded_file_bytes: 8_192,
        }
    }
}

/// A cap that refuses a change. The `Display` text is for the model.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapError {
    /// A file is larger than the file cap.
    #[error(
        "The file {} would be {} bytes. The limit is {} bytes. Make the content shorter or split it into several files.",
        path.to_tool_path(), thousands(*size), thousands(*limit)
    )]
    File { path: MemoryPath, size: u64, limit: u64 },
    /// The store would hold more bytes than the store cap.
    #[error(
        "The memory would hold {} bytes. The limit is {} bytes. The memory holds {} bytes now. Delete or shorten files to make room.",
        thousands(*after), thousands(*limit), thousands(*now)
    )]
    StoreBytes { after: u64, limit: u64, now: u64 },
    /// The store would hold more files than the file count cap.
    #[error(
        "The memory would hold {} files. The limit is {} files. The memory holds {} files now. Delete files to make room.",
        thousands(*after), thousands(*limit), thousands(*now)
    )]
    StoreFiles { after: u64, limit: u64, now: u64 },
}

/// Format a number with `,` as the thousands separator.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl MemoryCaps {
    /// The one cap rule. For each measure, a change is refused only when the measure is over its
    /// limit after the change and the change makes it larger. So a store that is over a cap can
    /// always become smaller.
    pub fn check(&self, before: MemoryUsage, after: MemoryUsage) -> Result<(), CapError> {
        if after.bytes > self.max_store_bytes && after.bytes > before.bytes {
            return Err(CapError::StoreBytes {
                after: after.bytes,
                limit: self.max_store_bytes,
                now: before.bytes,
            });
        }
        if after.files > self.max_store_files && after.files > before.files {
            return Err(CapError::StoreFiles {
                after: after.files,
                limit: self.max_store_files,
                now: before.files,
            });
        }
        Ok(())
    }

    /// The file cap. It applies to each `Put`.
    pub fn check_file(&self, path: &MemoryPath, size: u64) -> Result<(), CapError> {
        if size > self.max_file_bytes {
            return Err(CapError::File {
                path: path.clone(),
                size,
                limit: self.max_file_bytes,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(files: u64, bytes: u64) -> MemoryUsage {
        MemoryUsage { files, bytes }
    }

    #[test]
    fn check_refuses_only_over_and_growing() {
        let caps = MemoryCaps {
            max_store_bytes: 100,
            max_store_files: 2,
            ..MemoryCaps::default()
        };
        assert_eq!(caps.check(u(1, 50), u(1, 100)), Ok(()));
        assert!(matches!(caps.check(u(1, 50), u(1, 101)), Err(CapError::StoreBytes { .. })));
        assert_eq!(caps.check(u(1, 200), u(1, 150)), Ok(()));
        assert_eq!(caps.check(u(1, 200), u(1, 200)), Ok(()));
        assert!(matches!(caps.check(u(2, 10), u(3, 10)), Err(CapError::StoreFiles { .. })));
    }

    #[test]
    fn file_cap_text_gives_exact_bytes_and_the_tool_path() {
        let path = MemoryPath::parse("/notes/user.md").unwrap();
        let caps = MemoryCaps::default();
        assert_eq!(caps.check_file(&path, 65_536), Ok(()));
        let err = caps.check_file(&path, 65_537).unwrap_err();
        assert!(matches!(err, CapError::File { .. }));
        let text = err.to_string();
        assert!(text.contains("65,537 bytes"), "{text}");
        assert!(text.contains("65,536 bytes"), "{text}");
        assert!(text.contains("/memories/notes/user.md"), "{text}");
    }

    #[test]
    fn thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_048_576), "1,048,576");
    }
}
