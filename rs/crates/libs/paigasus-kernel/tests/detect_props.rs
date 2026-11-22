// SPDX-License-Identifier: Apache-2.0
#![cfg(feature = "detectors")]
//! Properties of the host extraction: it never panics, and it is deterministic.

use paigasus_kernel::detect::{ToolInput, call_hosts};
use proptest::prelude::*;

const POOL: [&str; 14] = [
    "https:",
    "://",
    ":::::::::::::::::::::::::::",
    "///////////////////////",
    "[",
    "]",
    "\\u00",
    "\\ud800",
    "é",
    "日本",
    "http://a.example/",
    "https://B.example:80/x?y=1",
    "a b",
    "ftp:\\\\h.example",
];

fn json_text() -> impl Strategy<Value = String> {
    let leaf = prop::collection::vec(prop::sample::select(POOL.to_vec()), 0..6).prop_map(|v| v.concat());
    let value = leaf.clone().prop_map(|s| format!("\"{s}\""));
    let pair = (leaf, value.clone()).prop_map(|(k, v)| format!("\"{k}\":{v}"));
    prop_oneof![
        value.clone(),
        prop::collection::vec(pair, 0..5).prop_map(|p| format!("{{{}}}", p.join(","))),
        prop::collection::vec(value, 0..5).prop_map(|p| format!("[{}]", p.join(","))),
    ]
}

proptest! {
    #[test]
    fn call_hosts_never_panics(s in ".{0,400}", j in json_text()) {
        let _ = call_hosts(&ToolInput::Text(s.clone()));
        let _ = call_hosts(&ToolInput::Json(s));
        let _ = call_hosts(&ToolInput::Json(j));
    }

    #[test]
    fn call_hosts_is_deterministic(j in json_text()) {
        let (a, b) = (call_hosts(&ToolInput::Json(j.clone())), call_hosts(&ToolInput::Json(j)));
        prop_assert_eq!(a.iter().collect::<Vec<_>>(), b.iter().collect::<Vec<_>>());
    }
}
