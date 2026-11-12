# paigasus-gateway-core

The gateway's tool-call interception core (SMA-749, ADR-0027). It is a synchronous state
machine with no I/O: it reads model response bytes, finds every tool call, holds it until it
is complete, asks a decider, and rewrites blocked calls into a notice.

`paigasus-gateway` drives it. Design: `docs/superpowers/specs/2026-11-09-sma-749-tool-call-assembler-design.md`.

Not published.
