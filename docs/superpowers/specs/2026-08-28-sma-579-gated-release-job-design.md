# SMA-579 — Release activation D: the gated `release` job, npm activation, and the re-founded release guard

Fourth increment of SMA-407. Input specs: §7/§9 of
`docs/superpowers/specs/2026-08-22-sma-407-release-activation-design.md` (the umbrella) and
**§9 of** `docs/superpowers/specs/2026-08-28-sma-578-maturin-wheel-matrix-design.md`, which is a
reviewed specification for this issue's PyPI half, not a sketch.

**Status:** design. Nothing here is implemented yet.

---

## 0. What this issue does, and what it deliberately does not

It ships the **complete but inert** irreversible half of the release path: tags, crates.io, PyPI
and npm, behind `vars.PAIGASUS_RELEASE_ENABLED == 'true'`. SMA-580 flips that variable after its
pre-flight. **This issue publishes nothing.**

It also settles three decisions that SMA-578 refused to make by default, because making them by
default is how they would have been made wrongly:

1. the napi ↔ release-plz **tagging boundary** (§2),
2. the **crates.io credential mechanism** (§4),
3. `py/packages/paigasus-proto`'s **PyPI ownership** (§5.3).

### Scope decision (2026-08-28)

The six sub-scopes were offered for splitting — specifically, moving the npm *publish path* to a
follow-up issue, since `prebuild.yml` is not a reusable workflow and converting it is a subsystem
of its own. **Sven chose all six in one PR.** This spec therefore carries the full scope. The
concern is recorded here rather than silently dropped: this is a large change touching two
workflows, two `package.json` files, `rs/release-plz.toml`, and a 4,745-line gate script.

### Not in scope

`@paigasus/kernel` / `@paigasus/proto` npm publish. There is no JS emit anywhere in `ts/` — every
build task is `tsc --noEmit` — so those need a TypeScript build pipeline first. Tracked separately.

---

## 1. The job graph

```
.github/workflows/release.yml        on: push → branches: [main]

  release-pr                                          ← existing, UNTOUCHED
  plan          if: vars.PAIGASUS_RELEASE_ENABLED == 'true'
                  release-plz release --dry-run --output json  → outputs.releases
  wheels        needs: plan   if: fromJSON(needs.plan.outputs.has_releases)
                  uses: ./.github/workflows/wheels.yml
  prebuild      needs: plan   if: fromJSON(needs.plan.outputs.has_releases)
                  uses: ./.github/workflows/prebuild.yml
  release       needs: [plan, wheels, prebuild]       environment: release
                  release-plz release                 ← FIRST IRREVERSIBLE STEP
  publish-pypi  needs: [wheels, release]              environment: release
  publish-npm   needs: [prebuild, release]            environment: release
```

### 1.1 Why this order

**Everything reversible precedes the first irreversible step** (SMA-578 review **B3**). The first
draft of the umbrella ran `release → wheels → publish-pypi`, so `release-plz release` completed the
crates.io upload *and* cut the tags before a single wheel was built. A failure in the six-leg
matrix — a zig regression, a runner image change — would then leave crates.io permanently
published, tags permanently cut, and `paigasus-kernel` missing from PyPI while pinning
`paigasus-py-bindings==X.Y.Z`.

Nothing forces the bad order: the release commit on `main` already carries the bumped versions, so
wheels can be built before release-plz runs. **`release.yml` must carry this rationale as a
comment**, because the order looks arbitrary otherwise and a future editor would "simplify" it.

### 1.2 Why `plan` exists

`release-plz release` is idempotent — it publishes only packages not yet on the registry at that
version — so the standard two-job pattern runs it on *every* push to `main`. Without `plan`, every
such push would also build a 6-leg wheel matrix and a 6-leg napi matrix, to learn there is nothing
to release. `plan` runs the cheap dry-run first and short-circuits the expensive legs.

`plan` is itself gated on `PAIGASUS_RELEASE_ENABLED`, so the entire graph below `release-pr` costs
nothing until SMA-580 flips it.

**`plan` is also the only job carrying the gate directly**, and that is deliberate rather than
incidental. `wheels`, `prebuild`, `release`, `publish-pypi` and `publish-npm` are all gated
*transitively*, through an unbroken `needs:` chain rooted at `plan`. This is exactly the topology
§6.2's verdict is written to reason about — a single direct gate plus a chain — so the guard's
`needs:`-walking is load-bearing on the real file, not merely exercised by fixtures. A guard that
only ever saw directly-gated jobs would have its most important code path untested in production.

### 1.3 The unmeasured premise, and its fallback

**`plan` assumes `release-plz release --dry-run` pushes no tags.** The CLI's own `--help` says only
*"Perform all checks without uploading"*, which is silent about tagging, and the command's summary
says it *"create[s] and push[es] upstream a tag … and then publish[es]"*. Those two sentences do
not settle whether `--dry-run` suppresses the tag half.

**This is a hard precondition of Task 1 of the implementation plan**, to be measured against the
pinned 0.3.158 in a throwaway clone with a fake remote — never against this repo's origin.

- **If `--dry-run` is non-mutating:** the graph above stands.
- **If it pushes tags:** delete `plan`; gate `wheels` and `prebuild` on
  `vars.PAIGASUS_RELEASE_ENABLED == 'true'` directly; accept a 12-leg build on every push to `main`
  once the flag is on. The B3 ordering is unaffected either way — only the cost is.

Recording both branches here means the fallback needs no re-design.

### 1.4 `concurrency:`

`release.yml:13-15` currently holds `concurrency: { group: release-pr, cancel-in-progress: false }`
at the **workflow** level. Adding a multi-leg matrix under that same group would serialize every
subsequent push to `main` behind a full wheel build.

**Design:** move the existing group to the `release-pr` job, and give the release path its own
job-level group. Neither may cancel in progress — a cancelled `release` job can leave crates.io
half-published, and a cancelled `release-pr` can leave the branch half-written (the reason the
existing setting is `false`).

`wheels.yml` and `prebuild.yml` carry their own workflow-level `concurrency` keyed on
`github.workflow`/`ref`/`event_name`. Called via `workflow_call` they inherit the **caller's**
`github.workflow`, so their groups do not collide with their own `push` runs. This must be
verified during implementation, not assumed.

---

## 2. The tagging boundary — **release-plz owns every tag**

`prebuild.yml:244-245` assigns this decision here in as many words. `napi prepublish` defaults
`ghRelease: true` and cuts a GitHub release plus a lerna-style tag
(`@paigasus/node-bindings@0.1.0`) from `package.json`. release-plz also cuts tags. Two tools
tagging one repo is precisely the ADR-0011 S3 failure mode — *"the tool owns every tag"*, singular —
and the SMA-385 trap that motivated it.

**Decision.** release-plz owns every tag. `napi prepublish` runs with `--no-gh-release` in the live
path, exactly as it already does in `prebuild.yml`'s dry-run. `@paigasus/wasm` publishes with a
plain `npm publish`, which never tags.

**`git_tag_name` stays unset**, i.e. the default `<package>-v<version>` — confirmed from
`release-plz release --help` at the pinned 0.3.158: *"create and push upstream a tag in the format
of `<package>-v<version>`"*. A group-collapsing name such as `v{{version}}` is **not** an option:
all four kernel-family packages share one version, so they would collide on a single tag name.

A release commit therefore cuts up to six tags, all at the same commit:

```
paigasus-kernel-v0.1.0          paigasus-proto-v0.1.0
paigasus-py-bindings-v0.1.0     paigasus-proto-derive-v0.1.0
paigasus-node-bindings-v0.1.0
paigasus-wasm-v0.1.0
```

Four of those are redundant by construction (the kernel family is version-locked). That redundancy
is accepted deliberately: the alternative is a collision.

**This decision is enforced, not merely documented.** §6's guard fails if any `napi prepublish`
invocation lacks `--no-gh-release`. Without that assertion the boundary would revert the first time
someone copied an invocation from upstream napi docs.

---

## 3. `release` — crates.io and tags

Runs `release-plz release` from `rs/`. Config and manifest discovery is CWD-relative with no upward
search (the same reason `release-pr` sets `working-directory: rs`).

`--output json` is captured and exposed as a job output. Per `--help` it *"prints the version and
the tag of the released packages"*; **its exact shape must be measured** against the pinned 0.3.158
before `publish-pypi` keys on it — the same discipline `release-pr` applied when it read
`release_pr()`'s `Option<ReleasePr>` from source rather than trusting `--help`. `publish-pypi` and
`publish-npm` both consume it, so an assumed shape would fail at the worst moment.

---

## 4. Credentials

**All credentials live in `release.yml` and nowhere else** (umbrella §7 review **M2**).
`prebuild.yml` and `wheels.yml` both carry a `pull_request` trigger, and same-repo PRs receive
repository secrets — so a contributor with push access could exfiltrate a registry token in a PR
that never merges. Registry tokens are the highest-value secret this project will hold and their
compromise is not reversible.

This is now also **externally enforced**: SMA-593 widens `repo:publish-metadata`'s P-D6 check from
`wheels.yml` alone to every workflow whose parsed triggers include `pull_request` or
`pull_request_target`. `release.yml` has neither, so it is correctly not a subject.

> **Constraint for every future editor of `release.yml`:** adding a `pull_request` or
> `pull_request_target` trigger to it makes it a P-D6 subject, and it will red — it genuinely reads
> secrets. That red is correct behaviour, not a bug. Do not add such a trigger, and do not reach for
> P-D6's `PR_CREDENTIAL_ALLOWED` escape hatch to silence it.

### 4.1 `environment: release`

All three publishing jobs (`release`, `publish-pypi`, `publish-npm`) declare
`environment: release`. crates.io's own Trusted Publishing guidance suggests it as hardening, which
is why it was requested — but it buys something the umbrella explicitly wanted and could not
otherwise get.

Umbrella §9 review **M12** accepts that the re-founded guard protects the *mechanism*, not the
*decision*: `PAIGASUS_RELEASE_ENABLED` is a repository variable any maintainer can flip in the UI
with no PR and no review. §9 names the remedy — *"the standard tool is a GitHub Environment with
required reviewers"* — and calls it an option, not part of that design. Declaring the environment
here **creates the place where that option can later be exercised**, without exercising it now.
Adding required reviewers to the `release` environment becomes a settings change, not a code change.

The environment also appears in the OIDC claim, so PyPI's and npm's trusted-publisher configs can
bind against it.

### 4.2 crates.io — OIDC, not a stored token

release-plz 0.3.158 authenticates with `CARGO_REGISTRY_TOKEN` (or `--token`). It has no native
OIDC support. crates.io Trusted Publishing needs an explicit OIDC→token exchange.

**Design:** exchange the job's OIDC identity for a short-lived crates.io token in a dedicated step,
and hand it to release-plz through the environment.

```yaml
permissions:
  id-token: write     # crates.io OIDC exchange
  contents: write     # release-plz pushes tags
steps:
  - uses: rust-lang/crates-io-auth-action@<pinned-sha>   # exact name + SHA to be verified
    id: cratesio
  - run: release-plz release
    env:
      CARGO_REGISTRY_TOKEN: ${{ steps.cratesio.outputs.token }}
```

No long-lived registry secret enters the repository. **The action's exact name, latest version and
output name must be verified during implementation** — this spec names the mechanism, not a
memorized identifier.

### 4.3 PyPI — trusted publishing

OIDC, `id-token: write`. **The claim binds to the *calling* workflow's filename**, so SMA-580
registers the pending publisher against **`release.yml`**, not `wheels.yml`. This is easy to get
wrong and produces a failure only at first publish.

### 4.4 npm — provenance

`npm publish --provenance`, `id-token: write`. A granular npm automation token is still required
for authentication (provenance attests origin; it does not authenticate), so `NPM_TOKEN` is the one
long-lived registry credential this design cannot avoid. It is confined to `publish-npm`'s own
`env:`, never a job- or workflow-level `env:`.

### 4.5 What SMA-580's pre-flight must create

| Kind | Name | Purpose |
|---|---|---|
| Repository variable | `PAIGASUS_RELEASE_ENABLED` | the gate; `'true'` activates |
| Environment | `release` | scopes OIDC claims; optional required reviewers |
| crates.io | Trusted Publisher | repo + `release.yml` (+ `release` environment) |
| PyPI | pending publisher ×3 | `paigasus-py-bindings`, `paigasus-kernel`, `paigasus-proto` — all against **`release.yml`** |
| Secret | `NPM_TOKEN` | granular npm automation token, publish scope, the two `@paigasus` packages |

The existing `PAIGASUS_BOT_APP_ID` / `PAIGASUS_BOT_PRIVATE_KEY` are unchanged and remain
`release-pr`'s.

---

## 5. `publish-pypi`

Consumes `wheels.yml`'s artifacts. That workflow was built for this consumer and its artifact names
are deliberate.

| Artifact | Contents |
|---|---|
| `wheel-<platform>` ×7 | `paigasus-py-bindings` platform wheels |
| `sdist` | `paigasus-py-bindings` sdist |
| `face-paigasus-kernel` | `paigasus-kernel` wheel + tar.gz |

### 5.1 Upload order

**`paigasus-py-bindings` first, then `paigasus-kernel`.** The face pins `==`, so the reverse order
leaves it uninstallable in the window between uploads (the derive→proto lesson, umbrella §3).

`wheels.yml` carries a standing comment warning that the face artifact is deliberately outside the
`wheel-*` namespace, precisely so that the natural implementation here — one `download-artifact`
with `pattern: wheel-*` and `merge-multiple: true`, then a single upload — cannot silently violate
that ordering. **Honour it: two downloads, two uploads.**

### 5.2 Idempotency and version binding

- **`skip-existing: true`** on every upload (review **M9**). The upload is multiple distributions;
  if a later one fails, a retry re-uploads the earlier ones and PyPI returns 400 *"file already
  exists"*, so an un-skipped retry can never succeed unaided. PyPI is delete-but-never-reuse.
- **Version binding** (review **M10**): assert the built wheel's version equals the version
  release-plz reports for `paigasus-py-bindings`, as a hard precondition of the upload. This is what
  stops a stale artifact — a re-run against a newer `main`, say — being published under a version it
  was not built from.

### 5.3 `py/packages/paigasus-proto` — **published here**

SMA-578 review **M8** required this be decided rather than left to omission. It is version-locked
with the **proto** family, its name is reserved on PyPI, and today no publish path uploads it. Left
unowned, every proto-family release burns a PyPI version that is never uploaded, so the Python
`paigasus-proto` permanently trails crates.io and can never be published at a matching version — an
irreversible skew introduced by doing nothing.

**Decision: publish it in this issue.** It is a pure-Python `uv_build` package
(`uv build --package paigasus-proto`), so the marginal cost is one build step.

**The complication is real and must be handled:** the proto family and the kernel family release on
**independent cadences**. `publish-pypi` therefore conditions each family's upload on that family
actually appearing in `release-plz release --output json`. A run that released only the kernel
family must not attempt a `paigasus-proto` upload, and vice versa.

`EXPECTED_PYPI_PUBLISHABLE` in SMA-578 §8 excludes `paigasus-proto` because it carries no
`[tool.paigasus] pypi = true` marker. Adding it to the publish path means adding that marker and
re-baselining that set.

---

## 6. The re-founded release guard

Umbrella §9 specifies it; SMA-576 scoped it but could not implement it (no job to guard); SMA-578
left it open for the same reason. **This issue has the job, so the guard lands here.**

### 6.1 Why the obvious rubric is wrong

SMA-578 review **B4** rejected the first draft's rubric, which transplanted
`assert_freshness_call_site`'s test — *"the `if:` is present, not defeated by a
`continue-on-error:` other than literal `false`, exit status not discarded"*. That rubric guards **a
check that must be able to report red**. This guard must **prevent execution**, and its bypasses
differ:

- `publish-pypi` is gated only *transitively* through `needs:`. An added `if: always()` or
  `if: !cancelled()` un-gates the upload while the pinned `release` guard stays byte-identical and
  green.
- `continue-on-error: true` on `release` does not suppress a red — it makes a **failed** release job
  count as success for `needs:`, so a failed crates.io publish still lets wheels reach PyPI.
- The verdict must find a **job-level** `if:` in a file already carrying seven step-level ones
  (`release.yml:45,63,77,81,85,106,125`). A grep-shaped verdict cannot tell them apart.

### 6.2 The verdict

> Every job that can reach a registry is gated on `PAIGASUS_RELEASE_ENABLED` — directly, or through
> an unbroken `needs:` chain from a gated job — and no such job carries `if: always()`,
> `if: !cancelled()`, or a `continue-on-error:` value other than the literal `false`.

Plus, from §2:

> No `napi prepublish` invocation omits `--no-gh-release`.

### 6.3 Implementation — real YAML, obtained through the pinned uv

SMA-578 §9.2 offered four routes and required one be picked and justified, on the premise that
*"`repo:publish-metadata` runs under `toolchain: 'system'`, where PyYAML is **not guaranteed** to be
importable."*

**That premise is measured false**, and the correction belongs in the record:

```
uv run --no-project --with 'pyyaml==6.0.3' python3 …      → 0.068s warm (this host)
```

`uv` is pinned in `.prototools`, `moon setup` installs it before `moon ci`, and CI restores the uv
cache first. The §9.2 constraint holds only for a bare `import yaml`. A version-pinned real parser
*is* reachable from a `toolchain: 'system'` task.

**Decision: `ci/actionlint/release_guard.py`, invoked through `uv run --no-project --with
'pyyaml==6.0.3'`.** Not a vendored parser.

The decisive argument is not the timing, it is the defect class. SMA-593 exists **because**
`ci/publish-metadata` hand-rolled a partial YAML scanner: it tracked quotes but not backslash
escapes, so `\"` closed a string early and the rest of the line vanished as a comment. That session
measured 14 distinct bypasses of it, including `permissions: write-all` and a YAML **alias**
(`x: &w write` … `id-token: *w`) whose value never stands next to its key and so is unreachable by
any text match. Writing a second hand-rolled indentation-aware scanner would recreate exactly the
defect class SMA-593 is removing, in a guard whose verdict depends on distinguishing job-level from
step-level keys and on walking `needs:` chains — the surface where those forms bite hardest.

*(Precision, since the looser claim was made once in discussion and corrected: the **merge-key**
form reds under a text checker too, but only **by accident** — the anchor definition appears
literally, so a regex matches the anchor, not the merged result. **Alias** is the genuinely
unreachable class. One unreachable, one caught-by-accident.)*

The two gates use **separate parse sites and an identical mechanism**. They share no predicate
(SMA-593's subject set is trigger-derived; this one's is reachability-derived), and a shared module
would make `ci/actionlint/**` an input to `repo:publish-metadata` and vice versa — widening both
gates' affectedness for no assertion gained, and forcing both `SELF_TASK_EXPECTED_GLOBS` entries to
move whenever either directory changed.

### 6.4 YAML 1.1 coercions — every one gets a fixture row

PyYAML is a YAML 1.1 parser and GitHub's schema collides with it in three places that land directly
on this verdict. All three were measured, not assumed:

| Source | Parses to | Consequence |
|---|---|---|
| `on:` (top-level key) | `True` (bool) | `doc.get("on")` returns `None`; must read `doc.get("on", doc.get(True))` |
| `if: false` | `False` (bool) | a job disabled outright is not the string `"false"` |
| `continue-on-error: false` | `False` (bool) | the "literal false" test must accept the **boolean**, not only the string |

Measured on `release.yml` itself: top-level keys come back as
`['name', True, 'concurrency', 'permissions', 'jobs']`, with `'on' in doc` **False** and
`True in doc` **True**.

A guard that tested `continue-on-error == "false"` would red on the correct spelling and pass on
`continue-on-error: true`-adjacent mistakes. Each row above becomes a named fixture.

### 6.5 Two strengthenings beyond the specified minimum

1. **Registry-reaching jobs are detected, then checked against a pinned expected set.** Detection
   (a step invoking `release-plz release`, `npm publish`, `napi prepublish`, `twine upload`, or a
   PyPI publish action) catches a publish step added to a *new* job. The pinned set catches
   detection silently ceasing to match — a renamed action, a reworded `run:`. Neither alone
   suffices: detection alone rots, and a pin alone misses new jobs.

   **The pin lives in `ci/actionlint/release_guard.py` itself**, as a module-level constant
   (`EXPECTED_REGISTRY_JOBS = {"release", "publish-pypi", "publish-npm"}`), compared to the detected
   set by **strict equality in both directions**. A detected job missing from the pin is a new
   publish path nobody declared; a pinned job no longer detected means detection rotted. Both red,
   with different messages, and both have fixture rows. Adding a genuine fourth publishing job is
   then a deliberate two-line change, which is the intent.
2. **Local reusable-workflow calls are followed.** A job whose `uses:` is
   `./.github/workflows/*.yml` is resolved and that workflow's own jobs are checked, so a publish
   step added to `wheels.yml` or `prebuild.yml` is reachable from `release.yml` and gated. P-D6
   covers those files' *credentials*; nothing today covers their *reachability*.

### 6.6 Guard-the-guard obligations

Per the repo's doctrine (`ci_targets.py`: *"That script cannot assert its own invocation"*), this is
a **new** verdict function against a **new** file:

1. A new `release_guard_self_test` table driving the verdict through pass and fail fixtures — one
   per bypass in §6.1, one per coercion in §6.4, one per strengthening in §6.5.
2. **`SELF_TEST_COUNT` 10 → 11.** The SMA-578 spec says "9 → 10"; **that is stale** — SMA-572
   already added `affected_smoke_block_self_test` as the tenth. Check 9 asserts invocations **and**
   definitions, so both must move together.
3. A whole-line `ACTIONLINT_SH_CALL_SITES` entry in `ci/affected-graph/ci_targets.py`, **at column
   0** (review **N5**): that haystack matches at column 0 deliberately, so a call site nested inside
   a function or an `if` cannot satisfy it.
4. Check 9's mutation battery is derived from `run_self_tests`' body, so an eleventh table makes a
   **twelfth** concurrent mutant. The measured cost tables in `ci/actionlint/README.md` and
   `moon.yml` must be **re-measured, not adjusted by estimate** — and re-measured the way that file
   already prescribes, via interleaved A/B sweeps rather than sequential min-of-N, which is invalid
   on this shared host.

### 6.7 What the guard still does not protect

Unchanged from umbrella §9 **M12**, and stated so it is not mistaken for a stronger claim: the guard
asserts the `if:` expression **exists and is not defeated**. The *decision* remains a repository
variable a maintainer can flip in the UI with no PR and no review. §4.1's `environment: release` is
where that gap can be closed later, by settings rather than by code.

---

## 7. npm activation

### 7.1 Package metadata

Both packages are `private: true` today, which `npm publish` refuses.

**`@paigasus/node-bindings`** — drop `private: true`. Everything else is already correct: it has
`publishConfig.access: public`, description, keywords, homepage, repository, engines.

**`@paigasus/wasm`** — drop `private: true`; add `publishConfig.access: public` (a scoped package
without it publishes **restricted**, unlike its sibling); add `description`, `repository`,
`homepage`, `keywords` to match.

### 7.2 The untracked `.wasm`

`package.json`'s `files` lists `paigasus_wasm_bg.wasm`, which is ignored by the **crate-local**
`rs/crates/bindings/paigasus-wasm/.gitignore:11` (`*.wasm`) — **not** the root `.gitignore`, which
has no wasm rule. Verified with `git check-ignore -v`.

The JS glue (`paigasus_wasm.js`, `paigasus_wasm_bg.js`, both `.d.ts` files) **is** tracked; only the
binary is absent from a fresh checkout. So `publish-npm` must run `wasm-pack` before publishing, or
it ships a package with no wasm binary — an artifact that installs cleanly and fails at import.

**An assertion is required, not just a build step:** after `wasm-pack` and before `npm publish`,
assert the `.wasm` exists and is non-empty. This is the same class as SMA-578's "a tag is not a
binary" rule — the publish would otherwise succeed and be irreversible.

### 7.3 `prebuild.yml` → reusable

`prebuild.yml` has `workflow_dispatch`, `push` and `pull_request` triggers but **no
`workflow_call`**, so `release.yml` cannot consume it the way it consumes `wheels.yml`. Add
`workflow_call`.

It stays credential-free — it keeps its `pull_request` trigger, so it is a P-D6 subject under
SMA-593 and must never declare `secrets:` or `id-token: write`. Publishing happens in `release.yml`,
which downloads `prebuild.yml`'s `prebuild-<platform>` artifacts.

`prebuild.yml`'s existing `permissions: contents: read` and its comment *"SMA-407 adds publish creds
at activation"* should be updated: creds are **not** added there, by design.

### 7.4 The publish steps

```
download prebuild-<platform> artifacts  →  $CRATE/artifacts
napi create-npm-dirs --cwd $CRATE
napi artifacts --cwd $CRATE --npm-dir npm
napi prepublish --no-gh-release --npm-dir npm --cwd $CRATE     ← §2's invariant
wasm-pack build (paigasus-wasm)  →  assert .wasm non-empty
npm publish --provenance  (@paigasus/wasm)
```

`--no-gh-release` is load-bearing and guarded (§6.2). `prebuild.yml`'s dry-run already passes it;
the live path must not "helpfully" drop it.

---

## 8. CI bookkeeping

- **No new `repo:*` gate.** This extends `repo:actionlint`, already in `ci.yml`'s `T=(…)` array. So
  **no `T` change and no CLAUDE.md marker edit** — which also avoids a conflict with two concurrent
  sessions editing `ci/affected-graph/ci_targets.py`.
- `repo:actionlint` already declares `inputs: ['**/*']`, pinned by
  `SELF_TASK_EXPECTED_GLOBS["actionlint"]`, so `ci/actionlint/release_guard.py` is covered with no
  input change. `repo:input-liveness` is satisfied for the same reason.
- **`ci/publish-metadata/**` is not touched by this issue** — it is SMA-593's, in flight in a peer
  session. `moon.yml`'s `repo:publish-metadata` `inputs` are likewise untouched, so
  `SELF_TASK_EXPECTED_GLOBS["publish-metadata"]` and its pinned `rs/.cargo/config.toml` entry do not
  move (which a third session's SMA-594 depends on).
- CLAUDE.md gains the release-path gotchas: the tagging boundary, the `--dry-run` measurement
  result, the three YAML 1.1 coercions, and the "never add a `pull_request` trigger to
  `release.yml`" constraint.

### 8.1 Concurrency with other sessions

Two peer sessions are active in this repo. Boundaries agreed directly with both:

| Session | Issue | Shared file | Resolution |
|---|---|---|---|
| paigasus-core-2b | SMA-593 | `ci/affected-graph/ci_targets.py` | they own `["publish-metadata"]`; this issue owns `SELF_SCHEDULED_GATES`, `ACTIONLINT_SH_CALL_SITES`, `["actionlint"]` |
| paigasus-core-2b | SMA-593 | `moon.yml` | they edit `repo:publish-metadata` inputs; this issue edits no task inputs |
| paigasus-core-3c | SMA-594/592/535 | `ci/affected-graph/ci_targets.py`, CLAUDE.md | disjoint regions; whoever lands second rebases |

---

## 9. Testing

Nothing in this design can be tested by running it — the whole point is that it does not execute.
So the evidence is structural:

1. **`release_guard_self_test`** — the fixture table of §6.6, driving the verdict through every
   bypass and coercion.
2. **A negative control.** The guard must be shown to report **red** on a deliberately broken
   `release.yml` (gate removed, `if: always()` added, `continue-on-error: true` added,
   `--no-gh-release` dropped), and green on the real one. A guard never observed reporting red is
   the "control that lies" failure this repo has paid for twice (SMA-542, SMA-530).
3. **`actionlint` over the new workflow structure**, including its `workflow_call` additions.
4. **`moon run repo:actionlint --force`** — the `--force` matters, since check 5's branch half reads
   git ref state that is in no input hash.
5. **The full gate graph** before pushing, per CLAUDE.md's marker-delimited command — not the
   per-project tasks, which do not run `repo:*` gates.
6. **`repo:version-lockstep`** — `package.json` edits touch two of its eighteen sites.

### 9.1 What cannot be tested pre-merge, and is therefore stated as risk

The publish path itself. No CI run can exercise a crates.io, PyPI or npm upload without publishing.
`--dry-run` covers part of the crates.io half; nothing covers the OIDC exchanges, which fail only
against a real registry with a real trusted-publisher registration. **SMA-580's pre-flight is where
that first executes, and it will be the first genuine test of §4.** This is inherent to the work,
not an omission.

---

## 10. Risks

| # | Risk | Mitigation |
|---|---|---|
| R1 | `release-plz release --dry-run` pushes tags, breaking `plan` | measured as Task 1; §1.3 fallback needs no re-design |
| R2 | `--output json`'s shape differs from the assumed one | measured before `publish-pypi` keys on it; §3 |
| R3 | `rust-lang/crates-io-auth-action`'s name/output verified from memory rather than upstream | §4.2 requires verification during implementation; the spec names the mechanism, not an identifier |
| R4 | PyPI trusted publisher registered against `wheels.yml` instead of `release.yml` | §4.3 states it explicitly; SMA-580's table repeats it |
| R5 | `wasm-pack` not run → package ships with no binary | §7.2's non-empty assertion, before publish |
| R6 | The 12-mutant battery pushes `repo:actionlint` past a tolerable runtime | re-measured, not estimated; if it regresses badly, the table's parallelism is the lever, not deleting fixtures |
| R7 | A future editor adds a `pull_request` trigger to `release.yml` | SMA-593's P-D6 reds it; §4's constraint block says so and forbids the allowlist escape |
| R8 | Merge conflict with two concurrent sessions in `ci_targets.py` | §8.1's agreed boundaries; different dict entries |
| R9 | The first `release` run tags six packages at once, four redundantly | accepted deliberately (§2); the alternative is a tag collision |

---

## 11. Open questions for the plan

1. Does `release-plz release --dry-run` push tags? **(R1 — blocks §1's shape.)**
2. What is `release --output json`'s exact schema at 0.3.158? **(R2 — blocks §5.)**
3. What is `rust-lang/crates-io-auth-action`'s current name, latest release SHA, and output name?
   **(R3.)**
4. Do `wheels.yml`/`prebuild.yml`'s workflow-level `concurrency` groups collide with the caller's
   when invoked via `workflow_call`? **(§1.4.)**
5. Does `napi prepublish` require an `NPM_TOKEN` in `env:`, or does it read `.npmrc`? Does it
   support `--provenance`, or must the platform packages publish without it?
