// SPDX-License-Identifier: Apache-2.0
//! Tests of the oracle models themselves (SMA-757).

mod support;

use serde_json::json;
use support::oracle::partial_json;

#[test]
fn partial_json_drops_an_unfinished_number() {
    assert_eq!(partial_json(r#"{"n":10."#).unwrap(), json!({}));
    assert_eq!(partial_json(r#"{"n":1e"#).unwrap(), json!({}));
    assert_eq!(partial_json(r#"{"n":-"#).unwrap(), json!({}));
    assert_eq!(partial_json(r#"{"a":"x","n":10."#).unwrap(), json!({"a":"x"}));
    assert_eq!(partial_json(r#"{"n":10"#).unwrap(), json!({"n":10}));
    assert!(partial_json(r#"{"n":1x}"#).is_err()); // invalid, not at the end of input
}
