# Review — SMA-405 python-semantic-release parity adapter (E3)

**Reviews:** [`2026-06-03-sma-405-python-semantic-release-parity-design.md`](./2026-06-03-sma-405-python-semantic-release-parity-design.md)
**Reviewer perspective:** staff engineer
**Date:** 2026-06-03
**Sources cross-referenced:** Linear SMA-405 (+ parent SMA-398), ADR-0011 (Notion), the python-semantic-release configuration docs, and the SMA-398 harness this builds on.

## Verdict

Tight, well-researched, and correctly scoped — ship it. This is the E3 adapter slice on top of SMA-398's harness, and it does the two hard things right: it identifies that PSR's 0.x defaults differ from release-plz's and *reconfigures PSR to match the canonical contract* (rather than documenting a divergence that would create real cross-language drift), and it adapts the fixture honestly to PSR's lack of path-based attribution by isolating each slot in its own git repo. I verified the load-bearing external facts and the foundation it rests on, and both hold.

The findings are minor. The only one worth acting on before merge is a cross-package config gap: the fixture derives its classification settings from `paigasus-ml` only, so `paigasus-workflows`'s PSR config is effectively untested — and the CI inputs make that a false reassurance rather than a caught drift.

## Verification I ran

- **PSR defaults are exactly as the spec claims.** The PSR docs confirm `allow_zero_version` flipped to **default `false` in v10** (left unset, PSR refuses 0.x and starts at `1.0.0`), and `major_on_zero` **defaults `true`** (breaking → `1.0.0` even in 0.x); set `false`, "major (breaking) releases will increment the minor digit while the major version is 0." So `major_on_zero = false` + `allow_zero_version = true` yields breaking→minor in 0.x (`0.1.0 → 0.2.0`), matching release-plz. The alignment is sound.
- **The foundation (ADR-0011) is solid, and it incorporated the SMA-398 review's two findings.** ADR-0011 S1 is now *hybrid* — "lockstep within the kernel and proto families; independent across unrelated packages" — and explicitly states it "refines Scoping §3 #4 / §4 by scoping their lockstep mandate to the kernel/proto families." That's the exact carve-out the SMA-398 review (F1) recommended, and it now *explicitly supersedes* the canonical §4 instead of silently diverging. S6 also replaced the degenerate `feat!:` row with `fix!:` / `fix: + BREAKING CHANGE` on a *patch base* as the discriminating 0.x case (annotating `feat!` as "degenerate in 0.x; discriminating at 1.0") — exactly the SMA-398 F2 fix. So SMA-405 inherits a sound contract, and its scoping (kernel/proto out, `-ml`/`-workflows` in) follows directly from S1/S2.

## What the spec gets right (calibration)

- **The "reconfigure to match, don't document the divergence" decision** is the correct reading of ADR-0011 S6, and the rejected alternative (let `-ml` classify breaking as `1.0.0`) is correctly rejected — it would reintroduce the cross-language drift the contract exists to prevent.
- **Honest fixture adaptation.** §2 is transparent that PSR has no path-based monorepo attribution, so it uses two independent git repos and documents that slot `b` tests "PSR invents no release without a qualifying commit" (not path→package attribution, which is a release-plz/cargo concern). `b` is a real package whose version is genuinely read — not a hardcoded constant. This is the right kind of intellectual honesty.
- **Config derived from the real file (F3 in the spec).** `build_fixture` greps the classification keys from the real `pyproject.toml` and fails loudly if `major_on_zero` is absent — so the harness exercises production settings and a config flip flows into the test. This is the same anti-drift pattern the SMA-398 review asked for, correctly applied.
- **CI granularity:** a separate `release-parity-py` task keyed on the py packages + `py/uv.lock` (so a PSR pin bump re-runs it) keeps the affected graph clean and matches SMA-398's `.prototools` rationale.

## Findings

### F1 — [Medium] The fixture derives config from `paigasus-ml` only; `paigasus-workflows`'s PSR config is untested, and the CI inputs make that a false reassurance

The spec (F3 note) acknowledges that `paigasus-ml` is the canonical derivation source and "a divergence in `paigasus-workflows`'s keys would not be caught by the fixture," deferring a cross-check guard. But the §5 task lists **both** packages' `pyproject.toml` as inputs — so editing `paigasus-workflows`'s `[tool.semantic_release]` *re-runs* `release-parity-py`, which then derives from `paigasus-ml` and passes green regardless of what `-workflows` now says. That's worse than not testing it: it's a green check that looks like it covered the edit. If someone later flips `major_on_zero = true` in `-workflows` (or drops `allow_zero_version`), `-workflows` would silently classify breaking changes as `1.0.0` at activation while the parity gate stays green.

This is the SMA-398 F3 fixture-drift concern recursed one level — within the Py adapter, across two packages. The fix is cheap and the spec already names it: add the cross-check guard **now**, not "later" — a ~3-line assertion that `-ml` and `-workflows` carry identical classification keys (`major_on_zero`, `allow_zero_version`). Given both configs are authored in this same slice, closing it here costs almost nothing and removes a false-green.

### F2 — [Low] The loud-failure derivation guard should cover `allow_zero_version`, not just `major_on_zero`

F3 says the fixture greps both `major_on_zero` and `allow_zero_version` but only that a missing `major_on_zero` fails loudly. A missing or `false` `allow_zero_version` is just as fatal to the contract — PSR would leave 0.x and jump to `1.0.0`, breaking every breaking-row assertion in a confusing way. The loud guard should assert **both** keys are present (and `allow_zero_version` is `true`), so a real-config that forgets `allow_zero_version` fails with a clear message rather than a misclassification.

### F3 — [Low] `version --print` is likely the better primary than write-then-read

The adapter runs `semantic-release version --no-commit --no-tag --no-push --no-changelog --skip-build` and then greps the written `project.version`, with `version --print` as the Risk #1 fallback. `--print` is the idiomatic PSR dry-run: it computes the next version and emits it with **no** file mutation, eliminating the "does `--no-commit` actually write `version_toml`?" uncertainty entirely. The spec chose write-then-read for symmetry with `release-plz.sh`'s manifest-reading `version` function — defensible — but the cleaner decoupling (`run_update` captures `--print` into a per-slot sentinel; `version` reads the sentinel) is the spec's own fallback and is less fragile. Worth promoting to primary unless the interface symmetry is load-bearing for a reason not stated.

### F4 — [Low] The "maturin byproduct" framing for the proto Py package is imprecise (inherited from ADR-0011)

The scoping rationale calls the kernel **and proto** Py packages "maturin byproducts of the Rust crate." That's accurate for the kernel (a maturin wheel of the PyO3 binding), but `paigasus-proto` (Py) is a **codegen** byproduct — betterproto output generated from `contracts/`, not a maturin build of the proto Rust crate. The conclusion (proto-py is out of PSR scope) is correct, so SMA-405 is unaffected. But the framing — carried from ADR-0011 S1/S2 — could misdirect the E-activate work: proto's version *propagation* is codegen-pipeline-driven (the generated package versioned to match the proto contract), not maturin-derived like the kernel, so its activation wiring differs. Worth a one-line correction in the ADR/spec so E-activate doesn't assume a maturin path for proto.

## Bottom line

Land it — the PSR alignment is verified correct, the fixture adaptation is honest, and it sits on an ADR-0011 that (gratifyingly) resolved the SMA-398 review's lockstep and 0.x-degeneracy findings. Before merge, close the one real gap: add the `-ml`/`-workflows` config-equality cross-check (F1) so the second package isn't a false-green, and extend the loud derivation guard to `allow_zero_version` (F2). Consider `version --print` as the primary computation path (F3), and fix the "maturin byproduct" wording for proto (F4).

## Sources

- Spec under review: `docs/superpowers/specs/2026-06-03-sma-405-python-semantic-release-parity-design.md`
- [Linear SMA-405 — python-semantic-release dormant config + Py parity adapter](https://linear.app/smaschek/issue/SMA-405/ci-python-semantic-release-dormant-config-py-semver-parity-adapter) (parent SMA-398)
- [Notion — ADR-0011: Polyglot versioning & release strategy](https://app.notion.com/p/373830e8fbaa8129a02bd1e0530d2475) (S1 hybrid lockstep; S6 `fix!`-discriminating 0.x table — both incorporating the SMA-398 review findings)
- [python-semantic-release configuration docs](https://python-semantic-release.readthedocs.io/en/latest/configuration/configuration.html) (`allow_zero_version` default `false` since v10; `major_on_zero` default `true`, `false` → breaking bumps minor in 0.x)
- SMA-398 harness: `ci/release-parity/{run.sh,cases.tsv,ecosystems/release-plz.sh}` (the ecosystem-agnostic core this adapter plugs into)
