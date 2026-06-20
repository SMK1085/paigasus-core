// SPDX-License-Identifier: Apache-2.0
//! Regenerates the committed parity corpus from the current kernel. Run via the
//! `repo:parity-corpus-drift` Moon task, which then `git diff --exit-code`s the result — so a
//! kernel edit landed without regenerating the corpus fails CI red (SMA-433).

use std::fs;

use paigasus_kernel_parity::{build_corpus, corpus_path, serialize};

fn main() -> std::io::Result<()> {
    let cases = build_corpus();
    let path = corpus_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serialize(&cases))?;
    eprintln!("wrote {} parity cases to {}", cases.len(), path.display());
    Ok(())
}
