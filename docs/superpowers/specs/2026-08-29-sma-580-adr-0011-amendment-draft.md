# ADR-0011 amendment — draft for Notion (SMA-580)

**This file is a draft, not the ADR.** ADRs live in Notion. This copy is committed only so the
amendment is reviewable in the pull request. The owner pastes it into
[ADR-0011: Polyglot versioning & release strategy](https://app.notion.com/p/373830e8fbaa8129a02bd1e0530d2475)
and then this file may be deleted.

Two edits are needed on that page: the **Status** line, and a new **Amendment** section appended
after the existing 2026-06-04 one.

---

## Edit 1 — the Status line

Replace:

> **Status:** Accepted *(amended 2026-06-03 · SMA-405: proto Py/TS clarified as buf-codegen byproducts of `contracts/`, not maturin; amended 2026-06-04 · SMA-406: S6 documented-exception clause + semantic-release TS 0.x exception)*

With:

> **Status:** Accepted *(amended 2026-06-03 · SMA-405: proto Py/TS clarified as buf-codegen byproducts of `contracts/`, not maturin; amended 2026-06-04 · SMA-406: S6 documented-exception clause + semantic-release TS 0.x exception; amended 2026-08-29 · SMA-580: activation shape + crates.io bootstrap exception to S3)*

---

## Edit 2 — the new amendment section

Append this after the existing `## Amendment — 2026-06-04 (SMA-406, E4)` section.

---

## Amendment — 2026-08-29 (SMA-580, E-activate)

SMA-407 activated the release path. This amendment records the four items its design reserved for
activation time, plus one exception that activation itself forced.

**1. S1 clarification — proto's lockstep is structural.**

S1 says the proto family carries one version driven by the contract source. The mechanism is now
concrete and worth stating, because it is not what "lockstep" usually implies. There is **no
contract version and no version-stamping step.** The generated Rust lives inside
`paigasus-proto/src/generated`, committed. A `contracts/` change regenerates it, which changes the
crate's files, and release-plz attributes the bump **by file path** (S5). Lockstep is therefore a
consequence of codegen plus path attribution, not of cross-tool coordination.

**2. S4 activation shape — `release-pr` live, `release` gated behind a repository variable.**

S4 said "dormant until real" without saying what the switch is. It is
`vars.PAIGASUS_RELEASE_ENABLED`. The `release-pr` job runs on every merge to `main` and is
deliberately **not** gated, because it must keep proposing the release PR while the flag is off.
That is only safe because it cannot reach a registry, which `ci/actionlint/release_guard.py`'s V7
now asserts: `release-pr` is in that guard's `UNGATED_JOBS`, and a member of that set carrying a
publish step fails the gate.

The guard **protects the mechanism, not the decision.** It proves the gating shape is intact. It
cannot prove the flag holds the right value, and nothing in CI reports that the release path is
inert.

**3. Decision G — deferred again, deliberately.**

The 2026-06-04 amendment routed a sub-1.0 lifecycle decision here: semantic-release ejects
`@paigasus/sdk` and `@paigasus/ui` to `1.0.0` on their first breaking change, while release-plz and
python-semantic-release stay in 0.x.

**It is not settled here, because its premise has not arrived.** Both packages are `private: true`
at `0.0.0`. semantic-release governs no package that publishes, so choosing between "accept early
1.0", "adopt the version-blind `releaseRules` clamp", and "reconsider the TS release tool" would be
choosing on hypotheticals.

**The trigger that reopens it: either package dropping `private: true`.**

**4. The temporary S1 exception — two family members at `0.0.0`.**

`@paigasus/kernel` and `@paigasus/proto` (the TypeScript faces) sit at `0.0.0` while their family
siblings move to `0.1.0`. This is a known, deliberate divergence from S1's one-version rule.

They rejoin at the family's **current** version, not at `0.1.0`. A family at `0.4.0` when they
rejoin takes them straight to `0.4.0`.

**5. NEW — the crates.io bootstrap exception to S3.**

S3 says the tool owns every tag and humans never hand-place a release tag. **Activation required
three versions the tool did not cut**, and this records why that is not a violation of S3.

*The constraint.* crates.io cannot pre-register a Trusted Publisher for a crate that does not
exist. RFC 3691 states it directly: *"A Trusted Publisher Configuration can only be created after
an initial manual publishing of a crate."* A `PENDING` state is listed under Future possibilities
and is not implemented. The `release` job authenticates only through
`rust-lang/crates-io-auth-action`, and all three publishable crates were unpublished, so the first
publish could not succeed.

*What was done.* `0.1.0-alpha.1` of `paigasus-kernel`, `paigasus-proto` and `paigasus-proto-derive`
was published by hand, from a scratch tree that was never committed and was deliberately not a git
repository. That created the crates, which made Trusted Publishing configurable. The seeds were
yanked once the real `0.1.0` verified.

*Why this is not the SMA-385 failure.* SMA-385 was caused by hand-placed **tags**. A manual tag
carries none of the metadata release-plz uses to track what has been released, so it silently stops
all future bumps. **The seed places no tag.** It writes only to the registry — which, with
`git_only` unset, is exactly the baseline release-plz reads. release-plz still owns every tag,
`0.1.0` included, and still cut all six of them plus the two GitHub Releases.

*The narrow reading of S3 that survives.* "The tool owns every tag" is unchanged and absolute.
"The tool publishes every registry version" was never stated in S3, and is now known to be
impossible for a first publish under Trusted Publishing. A future ecosystem with the same
bootstrap constraint may take the same exception, on the same terms: no tag, a pre-release version,
and a yank afterwards.

*References.* The procedure is
[`docs/ops/RUNBOOK-release-activation.md`](../../ops/RUNBOOK-release-activation.md) §4. The design
and its measurements are
[`docs/superpowers/specs/2026-08-29-sma-580-release-activation-e-design.md`](./2026-08-29-sma-580-release-activation-e-design.md).
