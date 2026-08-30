# SMA-602 Trusted Publishing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the `PYPI_API_TOKEN` and `NPM_TOKEN` bootstrap credentials from `release.yml`, move PyPI and npm to OIDC trusted publishing, and add a static gate so no publish credential can return unnoticed.

**Architecture:** A pure subtraction from one workflow file, plus one new check in an existing CI guard, plus documentation. The guard check is written **first**, so it reds against the unmodified `release.yml` and proves it bites before the removal makes it green. No new Moon task, no new workflow, no new dependency.

**Tech Stack:** GitHub Actions YAML; Python 3.12 (`ci/actionlint/release_guard.py`, run under `uv`); Markdown docs.

**Spec:** `docs/superpowers/specs/2026-08-30-sma-602-trusted-publishing-design.md`

## Global Constraints

- Every source file opens with an SPDX header. **No file in this plan is new**, so no new header is needed.
- Conventional commits with a workspace scope. All commits here use `ci(repo):` or `docs(repo):`, and every subject ends with `(SMA-602)`.
- Commit subjects start lowercase and are **≤100 characters**. Keep `#NNN` references out of the commit **body** — a `#NNN` line or a stray `token: value` line there fails `footer-leading-blank`.
- `PAIGASUS_BOT_APP_ID` and `PAIGASUS_BOT_PRIVATE_KEY` **must survive every edit**. `ci/workflow-credentials/run.sh:84` hard-fails if `release.yml` stops matching `${{ secrets.` (spec §5.5).
- `id-token: write` stays on `publish-pypi` and `publish-npm`.
- Prefix shell commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so moon/uv resolve to the repo-pinned versions.
- **Do not** run `npm --version` to check the npm version — `proto` shims `node` but not `npm`, so it resolves an ambient binary. The pinned npm is **11.13.0** (spec §2).
- Edits below cite line numbers from the **unmodified** file. Line numbers shift as you edit. **Anchor every edit on the quoted text, not the number.**

---

### Task 1: Add the publish-credential check (V10) to `release_guard.py`

**Files:**
- Modify: `ci/actionlint/release_guard.py` (add constants, a helper, two call sites, four fixture rows)

**Interfaces:**
- Produces: `publish_credential_violations(job: dict, job_id: str, name: str) -> list[str]`, called from both `check_main` and `check_called`.
- Consumes: the existing `infra()`, `FIXTURES` table, and the `--self-test` / `--fixture-count` entry points.

**Why both call sites.** `CLAUDE.md` records that V5 was once inlined in `check_main`, which `main()` runs on `argv[0]` only, so every called workflow got `check_called` — which had no V5 at all. Do not repeat that. The helper is invoked from both.

- [ ] **Step 1: Add the constants**

Add near the other module-level pattern constants (beside `PUBLISH_MARKERS`):

```python
# V10 (SMA-602): no registry publish credential anywhere in the release path. PyPI and npm
# authenticate through OIDC trusted publishing; crates.io already did. A reintroduced token
# publishes SILENTLY — npm's oidc.js never throws (its own doc comment says so), so a failed
# exchange falls through to whatever credential is configured, and the publish succeeds having
# used the token. Nothing else in this repository catches that: ci/workflow-credentials only
# inspects pull_request-triggered workflows, and release.yml is not one.
#
# This bans PUBLISH credentials BY NAME, never the `secrets` context as a whole.
# PAIGASUS_BOT_APP_ID and PAIGASUS_BOT_PRIVATE_KEY are legitimate and must keep working: an App
# installation token cannot come from a registry trusted publisher. A blanket ban would also red
# ci/workflow-credentials/run.sh's control row, which asserts release.yml still reads A secret.
BANNED_PUBLISH_CREDENTIALS = ("PYPI_API_TOKEN", "NPM_TOKEN", "NODE_AUTH_TOKEN")

# An `_authToken` written into any npmrc masks a broken OIDC exchange: npm's oidc.js sets its
# exchanged token at the 'user' config level, and a file token at that level is what publish.js
# falls back to when the exchange fails.
NPMRC_AUTH_TOKEN = "_authToken"
```

- [ ] **Step 2: Add the four failing fixtures**

Append to `FIXTURES`. `_OK_MAIN` already contains no credential, so it is the clean control.

```python
    # --- V10 (SMA-602): no registry publish credential in the release path -----------------
    ("V10 npm token in a step env", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - env:\n"
         "          NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}\n"
         "        run: npm publish\n"),
     "references NODE_AUTH_TOKEN"),
    ("V10 pypi token in a step with:", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - uses: pypa/gh-action-pypi-publish@v1\n"
         "        with:\n"
         "          password: ${{ secrets.PYPI_API_TOKEN }}\n"),
     "references PYPI_API_TOKEN"),
    ("V10 npmrc authToken written in a run:", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         '    steps: [{run: \'echo "//registry.npmjs.org/:_authToken=x" > "$HOME/.npmrc"\'}]'),
     "writes an npm _authToken"),
    # NEGATIVE CONTROL, and the most important row here. Without it, a future edit could ban the
    # whole `secrets` context and every other V10 row would still pass — while breaking the App
    # token mint and reding ci/workflow-credentials/run.sh's control row.
    ("V10 App secrets stay clean", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - env:\n"
         "          APP_ID: ${{ secrets.PAIGASUS_BOT_APP_ID }}\n"
         "          KEY: ${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}\n"
         "        run: release-plz release\n"),
     None),
```

- [ ] **Step 3: Run the self-test to verify the three violation rows fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py python3 ci/actionlint/release_guard.py --self-test
```

Expected: **FAIL.** The three violation rows report that the expected substring was not found, because `publish_credential_violations` does not exist yet. The negative-control row passes (it is already clean).

- [ ] **Step 4: Write the helper**

Add beside `napi_violations`:

```python
def publish_credential_violations(job: dict, job_id: str, name: str) -> list[str]:
    """V10: no registry publish credential anywhere in the release path.

    Invoked from BOTH check_main and check_called. Scoping it to check_main would repeat the
    SMA-579 V5 mistake, where a check living only in check_main left every CALLED workflow
    unguarded — main() runs check_main on argv[0] alone.

    Scans job env, step env, step run: bodies and step with: blocks. YAML comments are not in
    the parsed doc, so the explanatory comments in release.yml that NAME these tokens are
    invisible here — which is what lets those comments keep explaining the history.
    """
    out: list[str] = []

    def scan(text: str, where: str) -> None:
        for banned in BANNED_PUBLISH_CREDENTIALS:
            if banned in text:
                out.append(
                    f"{name}: job '{job_id}' references {banned} in {where}. PyPI and npm "
                    f"publish through OIDC trusted publishing (SMA-602). A token here would "
                    f"silently mask a broken exchange rather than fail. Remove it."
                )
        if NPMRC_AUTH_TOKEN in text:
            out.append(
                f"{name}: job '{job_id}' writes an npm {NPMRC_AUTH_TOKEN} in {where}. npm reads "
                f"that at the 'user' config level, which is exactly what masks a failed OIDC "
                f"exchange (SMA-602). Remove it."
            )

    for key, value in (job.get("env") or {}).items():
        scan(f"{key}: {value}", "the job env:")

    for step in job.get("steps") or []:
        if not isinstance(step, dict):
            continue
        for key, value in (step.get("env") or {}).items():
            scan(f"{key}: {value}", "a step env:")
        scan(str(step.get("run") or ""), "a step run:")
        with_block = step.get("with")
        if isinstance(with_block, dict):
            for key, value in with_block.items():
                scan(f"{key}: {value}", "a step with:")

    return out
```

- [ ] **Step 5: Call it from `check_main`**

Inside `check_main`'s `for job_id, job in jobs.items():` loop, immediately after the `napi_violations` call and **before** the `if job_id in UNGATED_JOBS:` block — V10 applies to every job, exempt ones included, exactly as V5 does:

```python
        # V10: applies to EVERY job, UNGATED_JOBS members included, so it runs BEFORE the
        # `continue` below. An exempt job with a publish token is the worst case, not an
        # excused one.
        out += publish_credential_violations(job, job_id, name)
```

- [ ] **Step 6: Call it from `check_called`**

Add the same call inside `check_called`'s per-job loop. If that function has no per-job loop, add one over `doc["jobs"].items()` that accumulates only this check.

- [ ] **Step 7: Run the self-test to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py python3 ci/actionlint/release_guard.py --self-test
```

Expected: **PASS**, all fixtures green.

- [ ] **Step 8: Prove the check bites the real file**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml; echo "exit=$?"
```

Expected: **exit=1** with violations naming `PYPI_API_TOKEN` (3), `NPM_TOKEN` (3), `NODE_AUTH_TOKEN` (3) and `_authToken` (3). This red is correct and expected — Task 2 clears it.

Record the exact violation count in the commit body. If it is **zero**, stop: the check is not wired in.

- [ ] **Step 9: Confirm the fixture floor still holds**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py python3 ci/actionlint/release_guard.py --fixture-count
```

Expected: **88** (84 before, plus 4). The floor pinned at `ci/actionlint/run.sh:4521` is `>= 20`, so no bump is needed.

- [ ] **Step 10: Commit**

```bash
git add ci/actionlint/release_guard.py
git commit -m "ci(repo): red a registry publish credential in release.yml (SMA-602)"
```

---

### Task 2: Remove the publish credentials from `release.yml`

**Files:**
- Modify: `.github/workflows/release.yml` — `publish-pypi` (3 sites) and `publish-npm` (3 steps)

**Interfaces:**
- Consumes: Task 1's V10 check, which currently reds against this file.
- Produces: a `release.yml` with no publish credential. Task 3 rewrites the comments that explain why.

**Do not touch the YAML comments in this task.** Task 3 owns them. Keeping the two apart lets a reviewer reject wording without rejecting the removal.

- [ ] **Step 1: Remove the three PyPI passwords**

Delete exactly these three lines. Each is the last line of its step's `with:` block:

```yaml
          password: ${{ secrets.PYPI_API_TOKEN }}
```

They sit under `packages-dir: dist-bindings`, `packages-dir: dist-face` and `packages-dir: dist-proto`. Leave `packages-dir:` and `skip-existing: true` in place. The action uses OIDC when a run supplies no `password` (`action.yml:9-11`).

- [ ] **Step 2: Rewrite the three npm publish steps**

Replace the first step's `env:` and the first two lines of its `run:`:

```yaml
        env:
          NPM_CONFIG_PROVENANCE: 'true'
          NPM_CONFIG_LOGLEVEL: verbose
        run: |
          set -euo pipefail
          pnpm exec napi prepublish --no-gh-release --npm-dir npm \
            --cwd ../../../rs/crates/bindings/paigasus-node-bindings
```

Replace the second step's (`Publish @paigasus/node-bindings (the main package)`):

```yaml
        env:
          NPM_CONFIG_LOGLEVEL: verbose
        run: |
          set -euo pipefail
          npm publish --provenance --access public
```

Replace the third step's (`Publish @paigasus/wasm`) with the identical block:

```yaml
        env:
          NPM_CONFIG_LOGLEVEL: verbose
        run: |
          set -euo pipefail
          npm publish --provenance --access public
```

Every step keeps an `env:` key, so no step loses the key entirely. `NPM_CONFIG_LOGLEVEL: verbose` is spec D7: `oidc.js` reports every failure below the default log level and `napi` pipes child output, so without it the operator sees a bare `ENEEDAUTH` in the least recoverable job.

- [ ] **Step 3: Verify the guard now passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml; echo "exit=$?"
```

Expected: **exit=0**, no output.

- [ ] **Step 4: Verify the App secrets survived**

```bash
grep -c 'secrets\.PAIGASUS_BOT' .github/workflows/release.yml
grep -n 'id-token: write' .github/workflows/release.yml
```

Expected: a non-zero count for the first, and `id-token: write` still present on both `publish-pypi` and `publish-npm`. If the count is 0, `ci/workflow-credentials` will red (spec §5.5).

- [ ] **Step 5: Verify no credential remains**

```bash
grep -n 'PYPI_API_TOKEN\|NPM_TOKEN\|NODE_AUTH_TOKEN\|_authToken' .github/workflows/release.yml
```

Expected: matches **only** inside YAML comment lines (those beginning with `#`). Task 3 rewrites those.

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci(repo): publish to PyPI and npm through OIDC trusted publishing (SMA-602)"
```

---

### Task 3: Rewrite the three comment blocks

**Files:**
- Modify: `.github/workflows/release.yml` — the blocks at `:37-52`, `:702-722` and `:931-936`

Each block states a reason this change makes false. **Ranges are exact and two carry keep-sub-ranges.** Anchor on quoted text.

- [ ] **Step 1: The trigger-security boundary (`:37-52`)**

Find the bullet reading:

```
  #   * Remove it — the job loses every credential, because ALL of them are ENVIRONMENT secrets
  #     rather than repository secrets: `NPM_TOKEN` and `PYPI_API_TOKEN` on `release-publish`, and
  #     `PAIGASUS_BOT_*` on both `release-pr` and `release-publish`. An OIDC exchange loses its
  #     environment claim too, which is what the crates.io trusted publishers require.
```

Replace with:

```
  #   * Remove it — the job loses its authorization. Since SMA-602 there is no publish token at
  #     all: crates.io, PyPI and npm each authenticate through OIDC trusted publishing, and every
  #     one of those publishers REQUIRES the `release-publish` environment claim. Dropping the
  #     `environment:` key drops that claim, so all three registries reject the exchange. The
  #     `PAIGASUS_BOT_*` App secrets remain ENVIRONMENT secrets on `release-pr` and
  #     `release-publish`, so a job that leaves the environment cannot mint a token either.
```

Do not write that the environment holds nothing. It still holds the two `PAIGASUS_BOT_*` secrets.

- [ ] **Step 2: The PyPI block (`:702-722`)**

**Keep `:706-710` verbatim** — the `pypi/warehouse#16920` paragraph. It still explains why a future project cannot use a pending publisher. **Do not touch `:723-727`**, which is executable YAML (`- name:`, `if:`, `uses:`, `with:`, `packages-dir:`).

Replace the opening two lines:

```
      # AUTH: a PyPI API TOKEN, temporarily, NOT the OIDC trusted publishing this job's
      # `id-token: write` was added for (SMA-580).
```

with:

```
      # AUTH: OIDC trusted publishing. This job supplies no `password:`, so
      # pypa/gh-action-pypi-publish mints an OIDC token instead (SMA-602).
```

Then replace the paragraph beginning `# The constraint binds PENDING publishers only.` with:

```
      # The constraint binds PENDING publishers only. So SMA-580 created all three projects with
      # an account-scoped token once, and SMA-602 then registered a NORMAL trusted publisher on
      # each — normal publishers may share a tuple, which is the ordinary monorepo case — and
      # deleted the token. Do not reintroduce one: release_guard.py's V10 reds on it, because a
      # token would MASK a broken exchange rather than fail. npm's OIDC helper never throws, so a
      # failed exchange falls through to any configured credential and publishes silently.
      #
      # A consequence worth knowing: with no `password:`, the action's `attestations` input
      # (default 'true', and effective only under Trusted Publishing) becomes live, so all three
      # projects now upload PEP 740 attestations. That needs no extra permission — they are
      # Sigstore-signed with the same OIDC token, not through GitHub's attestations API.
```

Keep the `ORDER IS LOAD-BEARING` paragraph unchanged.

- [ ] **Step 3: The npm block (`:931-936`)**

**Do not rewrite `:923-929`** — the `--no-gh-release` and `release_guard.py` V5 guidance. Replace only the `Auth:` paragraph:

```
      # Auth: npm Trusted Publishing (OIDC) cannot be configured before a package's first-ever
      # publish, and all three @paigasus/* packages are unpublished (measured: `npm view` 404s
      # for each) — so NPM_TOKEN stays for now, becoming unnecessary after the first release.
      # Confined to this step's own env (never job- or workflow-level) and written to the HOME
      # npmrc rather than the committed ts/.npmrc so every per-target `npm publish` napi shells
      # out to, regardless of its own cwd under --npm-dir, resolves it.
```

with:

```
      # Auth: npm Trusted Publishing (OIDC), one configuration per package. npm cannot configure
      # it before a package's first-ever publish (npm/cli#8544 is still open), which is why
      # SMA-580 needed a token and SMA-602 could remove it only afterwards.
      #
      # `napi prepublish` shells out to `npm publish` once per platform directory, each with its
      # own cwd. Those inherit OIDC correctly: @napi-rs/cli spawns them with `env: process.env`,
      # so both ACTIONS_ID_TOKEN_REQUEST_* variables reach every child (measured, 3.7.2
      # dist/index.js:3451-3455). The exchange is PER PACKAGE, so all nine @paigasus/* packages
      # need their own trusted publisher.
      #
      # NPM_CONFIG_LOGLEVEL: verbose is load-bearing, not debug residue. npm's oidc.js reports
      # every failure at verbose or silly and never throws, and napi pipes child stdio — without
      # it a misconfigured publisher surfaces as a bare ENEEDAUTH with no cause, in the one job
      # a fresh dispatch cannot repair (see RUNBOOK §7.4).
```

- [ ] **Step 4: Verify the workflow still parses and the guard still passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml; echo "exit=$?"
moon run repo:actionlint
```

Expected: `exit=0`, and `repo:actionlint` passes.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "docs(repo): record why release.yml carries no publish token (SMA-602)"
```

---

### Task 4: Rewrite the runbook

**Files:**
- Modify: `docs/ops/RUNBOOK-release-activation.md` — lines 35, 61, 175-176, §5.1, §5.2, line 644, §8

- [ ] **Step 1: The step table (line 35)**

Change the step C row so it no longer presents the tokens as the end state. Append to its label: `— tokens replaced by trusted publishing in SMA-602`.

- [ ] **Step 2: The environment table (line 61)**

Current:

```
| `release-publish` | `main` only | none | `NPM_TOKEN`, `PAIGASUS_BOT_APP_ID`, `PAIGASUS_BOT_PRIVATE_KEY` |
```

Replace with:

```
| `release-publish` | `main` only | none | `PAIGASUS_BOT_APP_ID`, `PAIGASUS_BOT_PRIVATE_KEY` |
```

Add directly beneath the table:

```
**Corrected 2026-08-30 (SMA-602).** This row previously listed `NPM_TOKEN` and omitted
`PYPI_API_TOKEN`, which was also present. Both are removed by SMA-602, so the row is now
accurate in both directions.
```

That row never listed `PYPI_API_TOKEN`. This is a correction, not a trim.

- [ ] **Step 3: The credential cross-check (lines 175-176)**

Current:

```
| `publish-pypi` | `release-publish` | `PYPI_API_TOKEN` | OK |
| `publish-npm` | `release-publish` | `NPM_TOKEN` | OK |
```

Replace with:

```
| `publish-pypi` | `release-publish` | none — OIDC trusted publishing | OK |
| `publish-npm` | `release-publish` | none — OIDC trusted publishing | OK |
```

- [ ] **Step 4: §5.1 (npm) and §5.2 (PyPI)**

Rewrite both to describe the trusted publishers as the steady state. Each configuration names owner `SMK1085`, repo `paigasus-core`, workflow `release.yml` (with the extension) and environment `release-publish`. Record three operational facts:

```
**Registration needs an OTP-capable login.** The npm trust commands wrap their registry call in
`otplease`, and 2FA-bypass granular tokens already lost the ability to change trusted-publishing
configuration (§5.1). The Automation token cannot do this. Use an interactive login or the web UI.

`npm trust github <pkg> --file release.yml --repository SMK1085/paigasus-core \
  --environment release-publish` registers one package. It also honours `--dry-run`.

`npm trust list <pkg> --json` reads the configuration back — an authenticated
`GET /-/package/<pkg>/trust` that needs NO GitHub Actions context, so it verifies all nine from a
laptop.
```

For PyPI, keep the pending-publisher history and add that three **normal** publishers replaced the token.

- [ ] **Step 5: Line 644, inside §7.4 — the dangling credential reference**

Current:

```
It needs a credential that bypasses 2FA — the **Automation** token, the same one in `NPM_TOKEN`.
```

Replace with:

```
It needs a credential that bypasses 2FA — the **Automation** token. SMA-602 deleted the
`NPM_TOKEN` secret from `release-publish`, but the underlying npm Automation token is deliberately
KEPT outside CI precisely so this recovery stays possible. **That token class loses direct publish
in January 2027**, after which this procedure stops working and this repository has no npm
hand-recovery path at all.
```

§7.4 keeps its procedure otherwise unchanged.

- [ ] **Step 6: §8, the tracked removals**

Replace the two-row table with:

```
| Item | Status |
| --- | --- |
| `PYPI_API_TOKEN` | **Discharged by SMA-602.** Three normal trusted publishers replaced it. Revoke the token on PyPI only AFTER a release has published through OIDC — it is the only credential that can publish to PyPI by hand, and PyPI has no documented hand-recovery procedure |
| `NPM_TOKEN` | **Discharged by SMA-602.** Nine trusted publishers replaced it. The secret is deleted; the underlying Automation token is kept outside CI as §7.4's recovery credential, until its January 2027 expiry |
```

Then replace `**No gate enforces either.**` with:

```
**A gate now enforces both.** `ci/actionlint/release_guard.py`'s V10 reds if `release.yml`
reintroduces `PYPI_API_TOKEN`, `NPM_TOKEN`, `NODE_AUTH_TOKEN` or an npmrc `_authToken` write. It
bans those BY NAME, never the `secrets` context as a whole — `PAIGASUS_BOT_*` must keep working,
and `ci/workflow-credentials/run.sh:84` separately asserts that release.yml still reads a secret.
```

Leave the `WITHDRAWN (SMA-603)` paragraph untouched.

- [ ] **Step 7: Verify no stale claim survives**

```bash
grep -n 'PYPI_API_TOKEN\|NPM_TOKEN' docs/ops/RUNBOOK-release-activation.md
```

Expected: every remaining match is historical or an explicit removal record. No line may still present a token as the current mechanism.

- [ ] **Step 8: Commit**

```bash
git add docs/ops/RUNBOOK-release-activation.md
git commit -m "docs(ops): describe the OIDC steady state in the release runbook (SMA-602)"
```

---

### Task 5: Annotate the SMA-580 spec

**Files:**
- Modify: `docs/superpowers/specs/2026-08-29-sma-580-release-activation-e-design.md` — §5.2 and §9.1

The spec is a dated record of a past decision. **Keep its text.** Add a note to each section only.

- [ ] **Step 1: Add the note to §5.2**

Insert immediately under the §5.2 heading:

```
> **SUPERSEDED IN PART, 2026-08-30 (SMA-602).** The `NPM_TOKEN` this section specifies was a
> bootstrap credential. All nine `@paigasus/*` packages now exist, so npm Trusted Publishing is
> configurable and has replaced it. The secret is deleted from `release-publish`. The text below
> is kept as the record of the token-era decision, not as current instruction. See
> `docs/superpowers/specs/2026-08-30-sma-602-trusted-publishing-design.md`.
```

- [ ] **Step 2: Add the note to §9.1**

Insert immediately under the §9.1 heading:

```
> **DISCHARGED, 2026-08-30 (SMA-602).** Both tracked removals are done: PyPI and npm now publish
> through OIDC trusted publishing, and `ci/actionlint/release_guard.py`'s V10 gates their return.
> One caveat this section could not know: the PyPI token must be revoked only AFTER a release has
> published through OIDC, because it is the only credential that can publish to PyPI by hand. See
> `docs/superpowers/specs/2026-08-30-sma-602-trusted-publishing-design.md` §6.3.
```

- [ ] **Step 3: Verify only two lines were added per section**

```bash
git diff --stat docs/superpowers/specs/2026-08-29-sma-580-release-activation-e-design.md
```

Expected: insertions only, **zero deletions**. If deletions appear, the original text was rewritten — revert and redo.

- [ ] **Step 4: Commit**

```bash
git add docs/superpowers/specs/2026-08-29-sma-580-release-activation-e-design.md
git commit -m "docs(repo): mark the SMA-580 token sections superseded by trusted publishing (SMA-602)"
```

---

### Task 6: Run the full gate graph

**Files:** none modified unless a gate reds.

The per-project tasks do not run the repo-level gates. Run the graph the way CI does.

- [ ] **Step 1: Run the full target list**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials --base origin/main \
  --include-relations
```

Expected: PASS.

**Two known traps while reading a failure.** The three `repo:release-parity*` gates abort **rc=2 (inconclusive), not rc=1**, inside an agent session, because `proto` emits NDJSON on stdout. Unset `AI_AGENT`, `CLAUDECODE` and `CLAUDE_CODE_ENTRYPOINT` before re-running one, or an inconclusive abort reads as a pass. And a sub-3s `repo:affected-smoke` failure is a known infrastructure abort — **capture the full output before re-running**, because a re-run passes and destroys the evidence.

- [ ] **Step 2: Diagnose any failure via the report, not by guessing**

Moon reports an unattributed "N failed". Read `.moon/cache/ciReport.json` to find which target went red.

- [ ] **Step 3: Confirm the two gates this change touches**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:actionlint
moon run repo:workflow-credentials
```

Expected: both PASS. `repo:workflow-credentials` is the one that reds if `release.yml` stopped reading any secret.

- [ ] **Step 4: Final read-through of the diff**

```bash
git diff origin/main --stat
git diff origin/main -- .github/workflows/release.yml
```

Confirm against spec §10: no `PYPI_API_TOKEN`, `NPM_TOKEN`, `NODE_AUTH_TOKEN` or `_authToken` outside comments; `id-token: write` on both jobs; `NPM_CONFIG_LOGLEVEL: verbose` on all three npm steps; `PAIGASUS_BOT_*` intact; no debug residue.

- [ ] **Step 5: Commit any gate fix**

Only if a gate required a change:

```bash
git add -A
git commit -m "ci(repo): satisfy <gate> after the trusted-publishing change (SMA-602)"
```

---

## Post-merge — owner actions, not code

These are **not** part of the PR. Spec §6, §7 and §10 own them. Recorded here so the executor does not attempt them:

1. Register 12 trusted publishers (3 PyPI, 9 npm) — **before** the merge.
2. Verify the nine with `npm trust list` — **before** the merge.
3. Delete `PYPI_API_TOKEN` and `NPM_TOKEN` from `release-publish` — after the merge.
4. **Revoke the PyPI token only after** a release has published through OIDC.

If the **second** PyPI registration fails the way the pending ones did, **stop and re-plan** — spec §6.1 flags that "normal publishers may share a tuple" is an assumption without an issue number behind it.

---

## Self-review

**Spec coverage.** §5.1 → Task 2 Step 1. §5.2 → Task 2 Step 2. §5.3 → Task 3. §5.4 → Task 1. §5.5 → Task 2 Step 4 and Task 6 Step 3. §5.6 → Tasks 4 and 5. §6, §7, §8 → owner actions, listed above and deliberately excluded. §9 risks → carried into the runbook (R2, R6) and the workflow comments (R7). §10 criteria 1-6 → Task 6 Step 4; criteria 7-8 → owner actions.

**Placeholders.** None. Every code step carries literal replacement text.

**Type consistency.** `publish_credential_violations(job, job_id, name) -> list[str]` matches `napi_violations`' shape and is called identically in both call sites. `BANNED_PUBLISH_CREDENTIALS` and `NPMRC_AUTH_TOKEN` are used only inside the helper. The fixture tuple shape `(name, kind, yaml, expected | None)` matches the existing `FIXTURES` declaration.

**One known ordering property.** Task 1 ends with the guard **red** against the real `release.yml`, by design — that is the proof it bites. Task 2 clears it. Do not "fix" Task 1's red by weakening the check.
