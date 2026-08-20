# error-registry — the canonical error-code single-site gate

`repo:error-code-single-site` runs `check.py --self-test && check.py --single-site`.

## What it gates

Every file under `rs/crates/*/*/src/**/*.rs` that spells one of the codes declared in
`contracts/proto/paigasus/common/v1/error.proto` must be on `check.py`'s `MANIFEST`.

That forces a **new emission site** to be registered and given a membership test. It is the
failure this repo already had twice: `system_retirement.rs` emitted two codes with no assertion at
all, and `authn.rs`'s membership test hand-restated the two codes it was meant to check, so a third
`envelope_rejection` branch would have escaped both.

## What it does NOT gate

**An undeclared code.** The scan greps for the strings the registry declares, so a site inventing
`"widget-jammed"` produces no hit and passes. Catching that would mean flagging kebab-case literals
by *shape*, which collides with `"content-type"`, `"application/json"` and `"paigasus-retryable"`.
The residual risk is bounded: a reason absent from the registry resolves through
`ErrorReason::from_wire_reason` on no consumer, so the code is dead on the wire regardless.

Also uncovered: codes composed at runtime (`format!("{prefix}-conflict")`), and a code added to an
already-listed file but outside the enum its guard enumerates.

## What it is NOT for

It does not check that a *removed* code is still emitted. Nothing needs to: both service crates
declare `test: deps: ['^:build']` in their own `moon.yml`, so a `contracts/` change already
schedules `paigasus-iam-rs:test` and `paigasus-gateway-rs:test`, and the membership tests run.
Verify with:

    export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
    printf 'contracts/proto/paigasus/common/v1/error.proto\n' | moon query tasks --affected --downstream deep

(Until SMA-528 this was aspirational: `^:build` schedules an upstream's build, it does not make a
consumer affected. What makes it true is `@group(upstreams)` — `paigasus-iam-rs`'s and
`paigasus-gateway-rs`'s `build`/`test`/`lint` now key on `paigasus-proto`'s sources, so a contracts
change that regenerates them selects both service crates' membership tests.)

## Adding a row

| Role | Use when | Needs |
|---|---|---|
| `emits` | the file puts a code on the wire | a membership test, named in the row; `check.py` asserts it still exists |
| `asserts` | test code that checks codes | a stated reason |
| `excluded` | the string is not a registry code here | a stated reason |

Every row must keep matching at least one hit — a stale row reds the gate rather than rotting.
