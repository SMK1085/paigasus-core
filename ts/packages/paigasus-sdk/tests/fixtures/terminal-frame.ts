// SPDX-License-Identifier: Apache-2.0
//
// The terminal SSE error frame, copied VERBATIM from the Rust constant TERMINAL_SSE_ERROR at
// rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs:63.
//
// Without this fixture the parser's test would hand-build the object it expects and could not fail
// when the gateway's frame changes — which is the one drift it exists to absorb. The drift check
// in tests/terminal-frame.test.ts reads the Rust file and asserts the two still agree, and
// moon.yml lists that file among paigasus-sdk-ts:test's inputs so the check RUNS on the PR that
// changes it.
export const TERMINAL_SSE_ERROR = 'data: {"error":{"message":"upstream stream error","type":"api_error","param":null,"code":"upstream-error"}}\n\n';

/** Where the Rust constant lives, relative to the repository root. */
export const CHAT_RS = 'rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs';
