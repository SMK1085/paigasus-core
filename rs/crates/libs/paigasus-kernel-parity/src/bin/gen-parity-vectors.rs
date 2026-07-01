// SPDX-License-Identifier: Apache-2.0
//! Regenerates every committed parity corpus from the current kernel. Run via the
//! `repo:parity-corpus-drift` Moon task, which then `git diff --exit-code`s the result — so a
//! kernel edit landed without regenerating a corpus fails CI red (SMA-433, SMA-448).

use std::fs;

use paigasus_kernel_parity::{build_corpus, build_prn_canonical_corpus, build_prn_cedar_corpus, build_uuid7_corpus, corpus_path, serialize};

fn write(name: &str, body: &str) -> std::io::Result<()> {
    let path = corpus_path(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, body)?;
    eprintln!("wrote {}", path.display());
    Ok(())
}

fn main() -> std::io::Result<()> {
    write("sum", &serialize(&build_corpus()))?;
    write("uuid7", &serialize(&build_uuid7_corpus()))?;
    write("prn_canonical", &serialize(&build_prn_canonical_corpus()))?;
    write("prn_cedar", &serialize(&build_prn_cedar_corpus()))?;
    Ok(())
}
