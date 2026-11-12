# paigasus-gateway-conformance

Test-only. Checks that the golden streams from `rs/crates/libs/paigasus-gateway-core/tests/golden`
parse with the pinned OpenAI and Anthropic Python SDKs: every allowed tool call arrives, no blocked
call arrives, and the notice is text (SMA-749 spec § 8.2). Not published.
