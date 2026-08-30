# SMA-602 — replace the PyPI and npm bootstrap tokens with trusted publishing

**Date:** 2026-08-30
**Issue:** [SMA-602](https://linear.app/smaschek/issue/SMA-602/release-replace-the-pypi-and-npm-bootstrap-tokens-with-trusted)
**Supersedes parts of:** `docs/superpowers/specs/2026-08-29-sma-580-release-activation-e-design.md` §5.2 and §9.1
**Revision:** 2 — reworked after an adversarial challenge. §14 lists what changed.

---

## 1. Summary

`.github/workflows/release.yml` publishes to PyPI and npm with two bootstrap credentials:
`PYPI_API_TOKEN` and `NPM_TOKEN`. Both are environment secrets on `release-publish`. Both exist
for one reason: neither registry can configure trusted publishing for a package that does not
yet exist.

All twelve packages now exist. This design removes both credentials, moves both registries to
OpenID Connect (OIDC) trusted publishing, and adds a static gate so no publish credential can
return unnoticed. crates.io already uses OIDC and does not change.

Two properties make the change necessary now. `NPM_TOKEN` is a 2FA-bypass Automation token, and
npm removes direct publish from that token class in **January 2027**. `PYPI_API_TOKEN` is
**account-scoped**, because the three PyPI projects did not exist when the token was minted.

---

## 2. Measured state

Each row below is a measurement made on 2026-08-30.

| Fact | Evidence |
| --- | --- |
| The three PyPI projects exist | `GET https://pypi.org/pypi/<name>/json` returns 200 for all three |
| The nine npm packages exist, all at 0.1.0 | `GET https://registry.npmjs.org/<name>` returns 200 for all nine |
| `release-publish` holds both tokens | `gh api …/environments/release-publish/secrets` lists `NPM_TOKEN` and `PYPI_API_TOKEN` |
| `publish-pypi` works from CI | Run `33264074393` published all three projects; run `33295917162` published `paigasus-proto` 0.1.1 |
| `publish-npm` has never published | Its one real attempt, run `33264074393`, failed with `pnpm: command not found` |
| The npm packages were published by hand | npm metadata reports `_npmUser: smaschek` and `_nodeVersion` 24.18.1 and 22.22.3, which are local versions, not the runner's 24.16.0 |
| No npm package carries provenance | No `attestations` key appears in any `dist` block |
| The pinned npm is **11.13.0** | `~/.proto/tools/node/24.16.0/lib/node_modules/npm/package.json:2`. The floor for trusted publishing is 11.5.1 |
| `pypa/gh-action-pypi-publish` v1.14.2 takes no `password` | `action.yml:9-11` — `required: false`, no default |
| That action enables PEP 740 attestations **by default** | `action.yml:83-87` — `default: 'true'`, and *"Only works with PyPI and TestPyPI via Trusted Publishing"* |
| The npm **CLI** exposes no registry-side enforcement setting | `npm/lib/commands/trust/index.js:9-13` exposes only `github`, `gitlab`, `circleci`, `list`, `revoke` |
| npm's **web UI** does offer one | A package's Publishing-access settings carry *"Require two-factor authentication and disallow tokens"*, and npm's own documentation recommends enabling it after a trusted publisher is configured |
| The environment claim is **optional** on npm | `trust/github.js:35-40` defines `environment` with `default: null` and **no** `required: true` — unlike `file`, which carries `required: true` at `:23-28`. A publisher registered without it accepts an exchange carrying no environment claim. PyPI's form accepts an empty environment too |
| npm 11.13.0 has **no** `--allow-publish` / `--allow-stage-publish` | `grep -r allow-publish` over `npm/lib/` returns nothing. Newer npm documents these flags; this version predates them |
| `release.yml` triggers on `push` to `main` | `.github/workflows/release.yml:27-30` |

**A measurement trap worth recording.** `proto` shims `node` but **not** `npm`. A bare
`npm --version` in this repository resolves an ambient nvm npm, not the Moon-managed one. Revision 1
of this spec reported 11.11.0 from that wrong binary. Read the version from the pinned node's
bundled `npm/package.json`, as the row above does.

The runbook records the hand-publish independently, in `docs/ops/RUNBOOK-release-activation.md` §7.4.

**Consequence.** The `publish-npm` job is unproven from end to end. Its first green run is real
acceptance evidence, not a formality.

---

## 3. The mechanism

### 3.1 How npm decides to use OIDC

`npm/lib/commands/publish.js:135` calls `oidc()` **before** it reads any credential.
`npm/lib/utils/oidc.js` then does this:

1. It stops unless the environment is GitHub Actions, GitLab or CircleCI.
2. It reads `ACTIONS_ID_TOKEN_REQUEST_URL` and `ACTIONS_ID_TOKEN_REQUEST_TOKEN`. A job needs
   `id-token: write` for the runner to set them.
3. It requests an ID token with audience `npm:registry.npmjs.org`.
4. It exchanges that token at `/-/npm/v1/oidc/token/exchange/package/<name>` (`oidc.js:120`).
   **The exchange is per package.** This is why nine npm configurations are necessary, not one.
5. On success it sets the exchanged token at the `'user'` config level (`oidc.js:140-141`), which
   overrides `~/.npmrc`.

**The helper never throws.** Its doc comment states this (`oidc.js:14-15`). Every failure path
returns `undefined` after a `log.verbose` or `log.silly` message — lines 36, 67, 97, 102, 110, 127,
132 and 170. The default log level hides all of them.

### 3.2 Why a token must not stay as a fallback

`publish.js:137` reads credentials after `oidc()` returns. `publish.js:155` throws `ENEEDAUTH` only
when no credential exists at all. Four configurations follow:

| Configuration | Result |
| --- | --- |
| No token, OIDC works | The package publishes through OIDC |
| No token, OIDC fails | `ENEEDAUTH`. The job reds and publishes nothing |
| Token present, OIDC works | The package publishes through OIDC. The exchanged token overrides the file |
| **Token present, OIDC fails** | **The package publishes through the token** |

Provenance cannot separate the rows. The workflow sets `NPM_CONFIG_PROVENANCE: 'true'`, so
`config.isDefault('provenance')` is false at `oidc.js:145` and the auto-enable branch at `:147`
never runs. Provenance is generated in every row.

**A correction to revision 1.** Revision 1 claimed row 4 is "not detectable at all". That is false.
`oidc.js:142` logs `Successfully retrieved and set token` at `log.verbose`. Setting
`NPM_CONFIG_LOGLEVEL: verbose` and grepping for that line makes row 4 detectable with the token
still in place — but only for the two direct `npm publish` steps. `napi prepublish` pipes each of
the seven platform children's stdio and forwards only their stdout, so the verbose line, which npm
writes to stderr, never surfaces there; only the failure path (row 2) is visible for those seven.

**The staged alternative that follows, and why this design rejects it.** One could keep both
tokens, add verbose logging, assert the OIDC line on a real release, then remove the tokens in a
second change. That is genuinely safer for the first release. It is rejected because the owner
chose a single undifferentiated change, and because the residual risk is bounded: with the tokens
gone, a broken publisher yields row 2 — a red job that uploads nothing and burns no version.
**The verbose logging is adopted anyway** (D7), because it is what makes a row-2 failure
diagnosable.

### 3.3 The per-platform publishes inherit the OIDC context

`@napi-rs/cli` 3.7.2 (`dist/index.js:3451-3455`) spawns each one as:

```js
execSync(`${npmClient} publish`, { cwd: pkgDir, env: process.env, stdio: "pipe" })
```

`env: process.env` passes the complete parent environment, which includes both
`ACTIONS_ID_TOKEN_REQUEST_*` variables. `cwd` selects which `package.json` and which `.npmrc`
npm reads. It does not affect environment inheritance.

`npmClient` defaults to `"npm"` (`dist/index.js:346`). Neither
`rs/crates/bindings/paigasus-node-bindings/package.json` nor
`ts/packages/paigasus-kernel/package.json` overrides it. So each child process is a real
`npm publish`.

This answers Risk 1 in the issue.

### 3.4 The committed `ts/.npmrc` does not apply

`ts/.npmrc` pins `registry=https://registry.npmjs.org/`. None of the actual `npm publish`
invocations run under `ts/`:

- The seven platform publishes run with `cwd` under `rs/crates/bindings/paigasus-node-bindings/npm/`.
- The main package publishes from `rs/crates/bindings/paigasus-node-bindings`.
- `@paigasus/wasm` publishes from `wasm-dist`.

They resolve npm's default registry, which is the same host. The OIDC audience is identical either
way.

### 3.5 PyPI turns on attestations

`pypa/gh-action-pypi-publish` v1.14.2 defaults `attestations` to `'true'`, and that input works
only through Trusted Publishing (`action.yml:83-87`). Today, with a token, it is inert. After this
change all three PyPI projects begin uploading **PEP 740 attestations**.

This is a benefit and a behaviour change. It needs no new permission: PEP 740 attestations are
Sigstore-signed with the same OIDC token, not through GitHub's attestations API, so
`id-token: write` remains sufficient. `publish-pypi` performs no checkout, so `contents` staying at
`none` is also correct.

---

## 4. Design decisions

| # | Decision | Reason |
| --- | --- | --- |
| D1 | Remove both tokens in one change | The owner's choice. §3.2 records the staged alternative and the bounded residual risk |
| D2 | Extend `ci/actionlint/release_guard.py` with a static check | It already parses `release.yml`, already runs in `repo:actionlint`, and is already in the `T` array, so it carries none of the five registry obligations a new `repo:*` gate would. **The decision stands; its premise is corrected (final review, Important 4).** Revision 1 said "the repository is the only place", citing the npm CLI. That is wrong: npm's web UI does offer *"Require two-factor authentication and disallow tokens"* per package (§2). So the repository is the only place **the CLI can reach**, and the only place a control can be version-controlled, reviewed and re-run on every PR — a registry setting is invisible to CI and readable only by hand. It is not the only place enforcement exists. **And that npm setting is not free here:** enabling it kills `RUNBOOK` §7.4 immediately, and D9's reason for keeping the Automation token with it (§9, R8) |
| D3 | Rewrite the runbook; annotate the SMA-580 spec | The runbook is a live operations document. The spec is a dated record of a past decision |
| D4 | Keep `id-token: write` on both jobs | It is what the trusted publishers use |
| D5 | Keep `NPM_CONFIG_PROVENANCE` and both `--provenance` flags | Provenance is orthogonal to authentication and uses the same ID token |
| D6 | Register the publishers before the PR merges | Registering early is free. Merging early breaks the next release. **Not** because a registered publisher is inert — §3.2 row 3 shows OIDC wins whenever it succeeds |
| D7 | Add `NPM_CONFIG_LOGLEVEL: verbose` to the three publish steps | `oidc.js` reports every failure below the default log level, and `napi` pipes child output. Without this the operator sees a bare `ENEEDAUTH` in the least recoverable job |
| D8 | Verify with `npm trust list`, not a dry-run publish | The dry run needs a GitHub Actions context that `release-publish` cannot grant off `main` (§8) |
| D9 | Keep the npm Automation token alive, outside CI | It is the credential `RUNBOOK` §7.4's hand-recovery needs. Deleting the GitHub secret does not revoke it |

---

## 5. The repository change

### 5.1 `publish-pypi`

Remove three inputs:

- line 729 — `password: ${{ secrets.PYPI_API_TOKEN }}` on "Publish paigasus-py-bindings"
- line 737 — the same input on "Publish paigasus-kernel"
- line 765 — the same input on "Publish paigasus-proto"

The action uses OIDC when a run gives no `password` (`action.yml:9-11`). The `permissions:` block
already declares `id-token: write` and does not change.

### 5.2 `publish-npm`

Remove six lines across three steps. Each step loses its `NODE_AUTH_TOKEN:` entry and its
`$HOME/.npmrc` write:

- "Publish the node addon and its platform packages" — lines 942 and 945
- "Publish @paigasus/node-bindings (the main package)" — lines 960 and 963
- "Publish @paigasus/wasm" — lines 987 and 990

Add `NPM_CONFIG_LOGLEVEL: verbose` to all three steps (D7).

No `.npmrc` replaces the removed writes. OIDC injects the exchanged token into npm's in-memory
`'user'` config. Note the removed lines **truncated** `$HOME/.npmrc`; after the change, whatever the
runner image ships at that path applies unmanaged. On `ubuntu-latest` that file does not exist, so
the effect is nil today, but the ownership has changed.

`NPM_CONFIG_PROVENANCE: 'true'` stays on the first step. `--provenance --access public` stays on
both direct publishes. Every step keeps an `env:` key, because every step keeps at least one
variable.

### 5.3 Three comment blocks change meaning

Each block states a reason that this change makes false. The ranges below are exact; revision 1
cited two of them wrongly.

**`release.yml:37-52` — the trigger-security boundary.** It argues that a job which drops its
`environment:` key loses every credential, naming `NPM_TOKEN` and `PYPI_API_TOKEN`. After this
change the argument gets stronger: no **publish** secret remains, so the boundary rests on the OIDC
environment claim, which all three registries require. Do not write that the environment holds
nothing — `PAIGASUS_BOT_APP_ID` and `PAIGASUS_BOT_PRIVATE_KEY` stay.

**`release.yml:702-722` — the PyPI pending-publisher block.** Rewrite it as the record of why
normal publishers replaced the token. **Keep `:706-710`**, the pypi/warehouse#16920 reference: it
explains why a future project still cannot use a pending publisher. Do not touch `:723-727`, which
is executable YAML.

**`release.yml:923-936` — the npm block.** Its claim that all three packages are unpublished is now
false. Rewrite that half and keep the npm/cli#8544 reference. **Do not rewrite `:925-929`**, which
carries the `--no-gh-release` and `release_guard.py` V5 guidance.

### 5.4 The new guard check

Add one check to `ci/actionlint/release_guard.py`. It asserts that `release.yml` declares no
registry publish credential:

- no `PYPI_API_TOKEN`, `NPM_TOKEN` or `NODE_AUTH_TOKEN` reference;
- no `_authToken` write to any `.npmrc`.

It must **not** ban the `secrets` context outright: `PAIGASUS_BOT_APP_ID` and
`PAIGASUS_BOT_PRIVATE_KEY` are legitimate and must stay.

Add fixture rows to `FIXTURES`: one healthy control that stays clean, and one row per banned form
that must red. `FIXTURES` currently holds **84** rows against the `>= 20` arity floor pinned at
`ci/actionlint/run.sh:4521`, so no floor bump is needed.

This discharges `RUNBOOK-release-activation.md:700`, which today reads "No gate enforces either".

### 5.5 Gate impact — checked, no break

| Gate | Effect |
| --- | --- |
| `ci/actionlint/release_guard.py` V1–V9 | Structural. They survive; §5.4 adds to them |
| `ci/actionlint/run.sh:5427`, `ci/affected-graph/ci_targets.py` | Pin the **invocation line**, not `release.yml`'s content |
| `ci/workflow-credentials/` | Its `PYPI_API_TOKEN` strings are synthetic self-test fixtures, not pins on `release.yml` |
| **`ci/workflow-credentials/run.sh:84`** | **Load-bearing.** It hard-fails with *"release.yml no longer reads a secret — re-baseline this control row"* if `release.yml` stops matching `${{ secrets.`. It stays green **only** because `PAIGASUS_BOT_*` remain (§11) |

That last row is now the single dependency keeping a control green. It is recorded here so a future
change that removes the App secrets knows to re-baseline it.

### 5.6 Documentation

**`docs/ops/RUNBOOK-release-activation.md` — rewritten.**

| Location | Change |
| --- | --- |
| line 35, the step table | Step C's `PYPI_API_TOKEN` and `NPM_TOKEN` entries |
| line 61, the environment table | Remove `NPM_TOKEN` **and correct an omission** — the row never listed `PYPI_API_TOKEN`, so this is a correction, not a trim |
| lines 175-176, the credential cross-check | `publish-pypi` and `publish-npm` need no secret |
| §5.1 and §5.2 | Describe the trusted publishers, not the tokens |
| **line 644, inside §7.4** | It names "the same one in `NPM_TOKEN`". Restate it as the npm Automation token held outside CI (D9), with no reference to a deleted secret |
| §8, the tracked removals | Both rows are discharged. Record the new gate |

**`docs/superpowers/specs/2026-08-29-sma-580-release-activation-e-design.md` — annotated.** §5.2
and §9.1 keep their text. Each gets a dated supersession note naming SMA-602.

---

## 6. The owner runbook

### 6.1 Register twelve trusted publishers

Every configuration names owner `SMK1085`, repository `paigasus-core`, workflow **`release.yml`**
(with the extension) and environment `release-publish`.

**Registration needs an OTP-capable login.** The npm trust commands wrap their registry call in
`otplease`. The account is `auth-and-writes`, and 2FA-bypass granular tokens **already** lost the
ability to change trusted-publishing configuration (`RUNBOOK-release-activation.md:393-395`). So
the Automation token cannot do this. Use an interactive login or the web UI.

**PyPI — three normal publishers:** `paigasus-py-bindings`, `paigasus-kernel`, `paigasus-proto`.

**npm — nine configurations:**

- `@paigasus/node-bindings`
- `@paigasus/wasm`
- `@paigasus/node-bindings-darwin-x64`
- `@paigasus/node-bindings-darwin-arm64`
- `@paigasus/node-bindings-win32-x64-msvc`
- `@paigasus/node-bindings-linux-x64-gnu`
- `@paigasus/node-bindings-linux-arm64-gnu`
- `@paigasus/node-bindings-linux-x64-musl`
- `@paigasus/node-bindings-linux-arm64-musl`

`npm trust github` takes `--file`, `--repository`, `--environment` and `-y`
(`trust/github.js:18-38`), and registers the global `dry-run` definition (`:42`), so each
configuration can be previewed before it is created.

**An assumption stated as an assumption.** §5.2 of the SMA-580 design asserts that *normal* PyPI
publishers may share an `(owner, repo, workflow, environment)` tuple. Only the *pending*
constraint carries an issue number (warehouse#16920). If the second PyPI registration fails the
same way the pending ones did, **stop** and re-plan the PyPI half. Do not proceed to the merge.

### 6.2 Delete the secrets

Do this **after** the PR merges. The workflow no longer reads them, so they are inert.

**BEFORE you delete either secret, confirm the token VALUE exists outside GitHub, and record
where** (final review, Important 2). GitHub secrets are write-only: deleting `NPM_TOKEN` does not
show you its value first, and there is no way to read it back afterwards. D9 keeps "the npm
Automation token alive" and §6.3 calls the PyPI token "the only credential that can publish to
PyPI by hand" — both statements are about the credential on the REGISTRY side, and both become
false if the only copy of the VALUE was the GitHub secret. A live token whose value nobody holds
recovers nothing.

For each of the two: open your password manager, confirm the value is stored there, and note the
entry name in `RUNBOOK` §7.4 (npm) and §8 (PyPI). If a value is not stored anywhere, do **not**
delete the secret. Mint a replacement on the registry first, store it, then delete.

Delete `PYPI_API_TOKEN` and `NPM_TOKEN` from the `release-publish` environment.

### 6.3 Revoke the PyPI token — only after acceptance

**Revoke the PyPI token on PyPI only after criterion 7 passes.** It is account-scoped, so it must
not linger, but it is also the only credential that can publish to PyPI by hand. Revoking it before
a release has published through OIDC removes the recovery path for the exact failure this change
introduces.

Deleting the GitHub secret does not revoke the token. Revocation is a separate action on PyPI.

**The npm Automation token is kept** (D9), out of CI. It is what `RUNBOOK` §7.4's hand-recovery
needs. Note its own expiry: that token class loses direct publish in January 2027 (§9, R6).

---

## 7. Order of operations

The order is the safety property.

1. Register all twelve trusted publishers (§6.1).
2. Verify the nine npm configurations with `npm trust list` (§8.1). This uploads nothing.
3. Merge the PR.
4. Delete both environment secrets (§6.2).
5. The next kernel-family release publishes through OIDC.
6. **After** that release succeeds, revoke the PyPI token (§6.3).

Step 1 must precede step 3: merging before registration breaks the next release. Step 6 must
follow step 5, per §6.3.

**On step 3.** Merging is a push to `main`, which starts `release.yml` (`:27-30`). A
workflow-and-docs PR bumps no version, so `ci/release-plan` finds nothing to release and the matrix
skips. `approve-release` gates the path in any case. No release starts from the merge itself.

---

## 8. Verification

### 8.1 npm — read the configuration back, from a laptop

`npm trust list <package> --json` performs an authenticated `GET /-/package/<name>/trust`
(`trust/list.js:35-47`) and reads back `repository`, `workflow_ref.file` and `environment`
(`trust/github.js:83-94`). It needs **no** GitHub Actions context.

Run it for each of the nine packages and confirm all three fields.

**Confirm `environment` specifically, and re-run this periodically** (final review, Important 5).
The field is optional at registration (§2, R9), and it is the entire trigger-security boundary now
that no publish secret remains. A registration that silently omits it, or a later edit through the
web UI that clears it, is invisible to CI. `npm trust list` is the only read-back this repository
has, it needs nothing but a laptop login, and it covers nine of the twelve configurations. The
three PyPI publishers cannot be read back at all.

**Why not a dry-run publish.** Revision 1 proposed `npm publish --dry-run` inside a scratch-branch
job. That cannot work. `release-publish`'s deployment branch policy is `main` only
(`RUNBOOK-release-activation.md:115`; `release.yml:46-47` states that a job entering it from
another ref fails). Dropping the `environment:` key does not help either: the ID token then carries
no `environment` claim, so the exchange fails against a publisher that requires one, and the error
is indistinguishable from a missing publisher. The claim also binds the workflow **filename**
(`trust/github.js:73-75`), so the probe would have to live in a file named `release.yml` on a
branch — the exact shape `release.yml:37-41` warns against. Two further problems made it unusable:
seven of the nine package directories do not exist in the tree (they arrive as the `npm-dirs`
artifact, `release.yml:841-845`), and every package is already published at its current version, so
`publish.js:159-171` throws under `--dry-run` regardless.

### 8.2 PyPI — no read-back, verified on the release

PyPI exposes no equivalent read-back and no OIDC dry run. Its three publishers are verified on the
next real release. A misconfigured publisher fails the upload and publishes nothing.

---

## 9. Risks and failure modes

**R1 — a partial release.** `release` publishes to crates.io and cuts tags before `publish-pypi`
and `publish-npm` run. A failure in either leaves crates.io tagged and a registry empty. This risk
exists today and this change does not add to it. Nothing publishes at the wrong version, and no
version is burnt.

**R2 — a re-dispatch cannot repair a failed publish job, on either registry.** The runbook records
this at §7.4 for npm. **It applies to PyPI identically**: `publish-pypi`'s `relinfo` reads
`needs.release.outputs.released` (`release.yml:609-646`) and gates every upload on it. Once the
release commit is tagged, `release-plz release` returns an empty `releases` array, the flags go
false, and the job goes **green having published nothing**. Re-running the job **inside the
original run** does work, because that run's `released` output is still populated. A fresh dispatch
does not.

**R3 — `publish-npm` has never completed.** Even with twelve correct publishers, it can fail for an
unrelated reason, exactly as it failed on `pnpm: command not found`. The next kernel-family release
is the first full exercise of this path.

**R4 — PyPI has no hand-recovery section.** The runbook carries one for npm (§7.4) and none for
PyPI. §6.3 keeps the PyPI token until acceptance so that a hand upload stays possible. Writing that
recovery procedure is out of scope here and is worth its own issue.

**R5 — the seven platform publishes are a loop.** `napi prepublish` publishes them one at a time.
A publisher missing on one package fails that package only. `@napi-rs/cli` catches "You cannot
publish over the previously published versions" and continues, so a re-run inside the original run
can complete the set.

**R6 — the npm recovery credential itself expires.** D9 keeps the Automation token for §7.4
recovery. That token class loses direct publish in **January 2027**. After that date the repository
has no npm hand-recovery path at all, and R2, R3 and R5 all lose their fallback. This deserves a
follow-up issue.

**R7 — PyPI attestations are new output.** §3.5. All three projects begin uploading PEP 740
attestations on the first OIDC release. The interaction with `skip-existing: true` is not measured:
a skipped file uploads no attestation. **A second, larger failure mode (final review, Minor 3):
attestations are now LIVE on all three uploads, and a failed attestation fails the UPLOAD.** The
Sigstore signing step runs before the upload, so a Sigstore outage, a rejected certificate, or an
attestation PyPI declines takes the whole `publish-pypi` job down — a failure class that did not
exist while `password:` was set, because the `attestations` input is effective only under Trusted
Publishing. It lands after crates.io has published (R1), and R2 says a re-dispatch cannot repair
it. Setting `attestations: false` on the three steps is the escape hatch if that happens.

**R8 — enabling npm's "disallow tokens" would kill §7.4 immediately.** §2 records that npm's web
UI offers *"Require two-factor authentication and disallow tokens"* per package, and npm's
documentation recommends it after a trusted publisher is configured. It is a real hardening
option, and this design does **not** take it: turning it on for the nine packages revokes the
Automation token's ability to publish them, so `RUNBOOK` §7.4's hand recovery dies **now** rather
than in January 2027, and D9's whole reason for keeping that token goes with it. The runbook says
so at §5.1. Revisit only once npm has a hand-recovery path that does not need a token, or once
R6's January 2027 deadline removes the choice anyway.

**R9 — the environment claim is an OPTIONAL field on twelve external configurations.** With no
publish secret left, the `release-publish` environment claim is the entire trigger-security
boundary for the npm and PyPI halves (`release.yml:44-60`). But the field is optional on both
registries: npm defines `environment` with `default: null` and no `required: true` (§2), and
PyPI's form accepts an empty environment. If ONE of the twelve registrations omits it, that
package's exchange succeeds with no environment claim, the repository stays green, and nothing
detects it. §8.1's `npm trust list` (RUNBOOK §5.1.2) catches an npm omission **once, by hand, before merge**; PyPI
has no read-back at all (§8.2). So the boundary rests on twelve external settings verified once.
The runbook adds a periodic `npm trust list` re-check (§8.1) for the nine npm ones; the three PyPI
ones remain unverifiable after registration.

### Rollback

If criterion 7 fails, re-adding `password:` needs a PyPI token. §6.3 keeps the existing one until
acceptance, and a **new project-scoped** token can be minted at any time now that the projects
exist — a strictly better credential than the account-scoped one being retired. On npm, D9 keeps
the Automation token until January 2027.

---

## 10. Acceptance criteria

1. `.github/workflows/release.yml` contains no `PYPI_API_TOKEN`, no `NPM_TOKEN`, no
   `NODE_AUTH_TOKEN` and no `_authToken` write.
2. Both jobs keep `id-token: write`. All three npm publish steps carry `NPM_CONFIG_LOGLEVEL: verbose`.
3. The three comment blocks in §5.3 state the current reason, and the keep-ranges are untouched.
4. `ci/actionlint/release_guard.py` reds on a reintroduced publish credential, proved by new
   fixture rows, and stays green on `PAIGASUS_BOT_*`.
5. The runbook describes the OIDC steady state, including the §7.4 credential restatement. The
   SMA-580 spec carries dated supersession notes.
6. `moon ci` passes on the full target list.
7. **Owner steps:** twelve publishers registered, `npm trust list` confirms all nine, both
   environment secrets deleted.
8. **Final evidence, after the next kernel-family release:** `publish-pypi` and `publish-npm` both
   complete with no publish secret in `release-publish`. Then, and only then, the PyPI token is
   revoked.

Criteria 7 and 8 land after the merge. The PR is complete without them. The issue is not.

---

## 11. Out of scope

- **crates.io.** It already uses `rust-lang/crates-io-auth-action` with OIDC.
- **npm staged publishing.** The fallback if OIDC proves unworkable. §10.2 of the SMA-580 design
  records the choice. It would add a manual 2FA approval to every npm release.
- **A PyPI hand-recovery procedure** (R4) and **an npm recovery path past January 2027** (R6). Both
  deserve their own issues.
- **`PAIGASUS_BOT_APP_ID` and `PAIGASUS_BOT_PRIVATE_KEY`.** These stay. An App installation token
  cannot come from a registry trusted publisher. §5.5 records that a control depends on them.

---

## 12. What the challenge changed

An adversarial review of revision 1 returned two blockers and nine major findings. The substantive
corrections:

| Finding | Change |
| --- | --- |
| §8.1's scratch-branch check cannot run — `release-publish` is `main`-only | Replaced with `npm trust list` (§8.1, D8) |
| Revoking the PyPI token before acceptance removes the only recovery | Split into §6.2 and §6.3; revocation moved to step 6 |
| "Not detectable at all" was false | Corrected in §3.2; verbose logging adopted as D7 |
| D6's "a registered publisher is inert" contradicted §3.1 | Fourth table row added; D6 restated |
| R2 named npm only | Extended to PyPI, with `release.yml:609-646` |
| §7.4's recovery depends on a token the spec deleted | D9 keeps it; runbook line 644 restated |
| No PyPI evidence | Two rows added to §2; attestations behaviour added as §3.5 and R7 |
| No gate-impact statement | §5.5, including the `workflow-credentials` dependency |
| D2 was a straw man | Reversed — the guard check is now in scope (§5.4) |
| npm version, two comment ranges, and the line-61 row were wrong | Corrected in §2, §5.3 and §5.6 |
