// SPDX-License-Identifier: Apache-2.0

//! The retry helper of the Docker test support. It needs no Docker.

mod support;

use std::cell::Cell;

#[tokio::test]
async fn a_start_that_fails_once_is_tried_again() {
    let tries = Cell::new(0usize);
    let out = support::retry_start(3, |n| {
        tries.set(n);
        async move { if n == 1 { Err("not ready".to_string()) } else { Ok(Some(n)) } }
    })
    .await;
    assert_eq!(out, Ok(Some(2)));
    assert_eq!(tries.get(), 2, "the start stops at the first success");
}

#[tokio::test]
async fn a_start_that_always_fails_gives_the_last_error_after_the_limit() {
    let out: Result<Option<usize>, String> = support::retry_start(3, |n| async move { Err(format!("fail {n}")) }).await;
    assert_eq!(out, Err("fail 3".to_string()));
}

#[tokio::test]
async fn a_skip_ends_at_once() {
    let tries = Cell::new(0usize);
    let out: Result<Option<usize>, String> = support::retry_start(3, |n| {
        tries.set(n);
        async { Ok(None) }
    })
    .await;
    assert_eq!(out, Ok(None));
    assert_eq!(tries.get(), 1);
}
