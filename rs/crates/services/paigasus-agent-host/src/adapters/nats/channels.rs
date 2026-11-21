// SPDX-License-Identifier: Apache-2.0

//! NATS subject filters: overlap and match (spec §9.1).

/// True when some subject can match both filters.
#[must_use]
pub fn filters_overlap(a: &str, b: &str) -> bool {
    let mut left = a.split('.');
    let mut right = b.split('.');
    loop {
        match (left.next(), right.next()) {
            // A `>` matches one or more tokens. The other side still has a token here, or it is `>` too.
            (Some(">"), Some(_)) | (Some(_), Some(">")) => return true,
            (Some(x), Some(y)) => {
                if x != "*" && y != "*" && x != y {
                    return false;
                }
            }
            (None, None) => return true,
            // One filter is longer. A `>` cannot match zero tokens, so there is no overlap.
            _ => return false,
        }
    }
}

/// True when the literal `subject` matches `filter`.
#[must_use]
pub fn subject_matches(filter: &str, subject: &str) -> bool {
    let mut f = filter.split('.');
    let mut s = subject.split('.');
    loop {
        match (f.next(), s.next()) {
            (Some(">"), Some(_)) => return true,
            (Some(ft), Some(st)) => {
                if ft != "*" && ft != st {
                    return false;
                }
            }
            (None, None) => return true,
            _ => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_table() {
        let rows = [
            ("iam.organization.>", "iam.organization.created", true),
            ("iam.*.created", "iam.organization.>", true),
            ("iam.organization.>", "iam.principal.>", false),
            ("a.b", "a.b.c", false),
            ("a.>", "a", false),
            (">", "x", true),
            ("a.*", "a.b", true),
            ("iam.>", "iam.organization.created", true),
            ("iam.*.created", "iam.organization.*", true),
            ("iam.*.created", "iam.organization.deleted", false),
            ("a.>", "a.>", true),
        ];
        for (a, b, want) in rows {
            assert_eq!(filters_overlap(a, b), want, "{a} vs {b}");
            assert_eq!(filters_overlap(b, a), want, "{b} vs {a}");
        }
    }

    #[test]
    fn match_table() {
        let rows = [
            ("iam.organization.>", "iam.organization.created", true),
            ("iam.organization.>", "iam.organization", false),
            ("a.*.c", "a.b.c", true),
            ("a.*.c", "a.b.d", false),
            ("a.*", "a.b.c", false),
            (">", "x.y", true),
            ("a.b", "a.b", true),
        ];
        for (f, s, want) in rows {
            assert_eq!(subject_matches(f, s), want, "{f} vs {s}");
        }
    }
}
