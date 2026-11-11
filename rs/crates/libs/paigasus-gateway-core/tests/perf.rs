// SPDX-License-Identifier: Apache-2.0
//! T-perf (spec § 8.3): 1 MiB of text-only stream, interception on, against a plain copy (off).
//! `#[ignore]`: it measures, it does not gate. Run it and record the numbers in the PR:
//! `cargo nextest run --release -p paigasus-gateway-core --test perf --run-ignored only --no-capture`

use std::time::Instant;

use bytes::Bytes;
use paigasus_gateway_core::{Action, Format, Interceptor, Limits};

#[test]
#[ignore = "a measurement, not a gate"]
fn one_mib_of_text() {
    let rec =
        "data: {\"id\":\"c\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lorem ipsum dolor \"},\"finish_reason\":null}]}\n\n";
    let n = (1 << 20) / rec.len();
    let body = rec.repeat(n);
    let chunks: Vec<&[u8]> = body.as_bytes().chunks(4096).collect();

    let t = Instant::now();
    let mut copied = 0usize;
    for c in &chunks {
        copied += Bytes::copy_from_slice(c).len();
    }
    let off = t.elapsed();

    let t = Instant::now();
    let now = Instant::now();
    let mut i = Interceptor::new(Format::OpenAiChat, Limits::defaults(), Bytes::new(), now);
    let mut emitted = 0usize;
    for c in &chunks {
        for a in i.push(c, now) {
            if let Action::Emit(b) = a {
                emitted += b.len();
            }
        }
    }
    let on = t.elapsed();

    assert_eq!(copied, body.len());
    assert!(emitted >= body.len());
    println!(
        "T-perf: {n} records, {} bytes; off {:?}, on {:?}, ratio {:.1}x",
        body.len(),
        off,
        on,
        on.as_secs_f64() / off.as_secs_f64().max(1e-9)
    );
}
