// SPDX-License-Identifier: Apache-2.0
//! The normal form of a tool name (§ 6.1.3).

/// `nfc(lowercase(nfc(trim(name))))`.
#[must_use]
pub fn normalise(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let folded: String = name.trim().nfc().collect::<String>().to_lowercase();
    folded.nfc().collect()
}

#[cfg(test)]
mod tests {
    use super::normalise;

    #[test]
    fn trim_nfc_and_case_fold() {
        assert_eq!(normalise("  Read_File\t"), "read_file");
        assert_eq!(normalise("cafe\u{301}"), normalise("caf\u{e9}"));
        assert_eq!(normalise("CAF\u{c9}"), "caf\u{e9}");
    }

    #[test]
    fn nfc_runs_again_after_lowercasing() {
        let s = normalise("\u{212b}");
        assert_eq!(s, "\u{e5}");
        assert!(unicode_normalization::is_nfc(&normalise("\u{130}X\u{301}")));
        assert!(unicode_normalization::is_nfc(&normalise("\u{1e9e}\u{323}\u{307}")));
        // No precomposed capital exists, so only the NFC after lowercasing makes the lowercase letter.
        assert_eq!(normalise("T\u{308}"), "\u{1e97}");
        assert_eq!(normalise("W\u{30a}"), "\u{1e98}");
    }

    #[test]
    fn normalise_matches_what_a_policy_literal_matches() {
        for raw in [" Read_File", "READ_FILE", "read_file\u{a0}"] {
            assert_eq!(normalise(raw), "read_file");
        }
    }

    #[test]
    fn normalising_twice_changes_nothing() {
        for raw in ["\u{130}X\u{301}", "\u{212b}", " Ab\u{301}C ", "\u{1e9e}"] {
            let once = normalise(raw);
            assert_eq!(normalise(&once), once);
        }
    }
}
