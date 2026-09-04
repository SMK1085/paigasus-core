# SMA-408 — CLA bot setup (cla-assistant)

## Problem

`paigasus-core` is public (since 2026-05-26, zero forks to date) and Apache-2.0
licensed. ADR-0007 requires a Contributor License Agreement in place *before* the
first external PR is accepted, because a retroactive CLA is awkward or impossible
once a contributor disappears. Nothing implements that today: `CONTRIBUTING.md:212-217`
still says the CLA is "automated via a bot — currently being set up".

The repo is public **now**, so the trigger condition the issue described ("wire it just
before the first external PR is expected") is already live. Nobody has to announce
themselves before opening a PR.

## Decision context

Settled by **ADR-0007 Amendment 1 (2026-09-03)**, written as part of this issue:

- **A CLA, not a DCO.** A DCO is an attestation, not a grant — it transfers no rights and
  cannot deliver the relicensing/dual-licensing optionality ADR-0007 exists to preserve.
  Twelve comparable open-core projects examined on 2026-09-03 — LiteLLM, Langfuse, n8n,
  Grafana, Temporal, PostHog, Supabase, Cal.com, Airbyte, Kong, Portkey, Helicone — of which
  five require a CLA, seven document no contributor agreement, and **none uses a DCO**.
  Method: `.github/workflows/` listing plus a case-insensitive grep of each `CONTRIBUTING.md`.
- **Hosted cla-assistant.io**, unchanged from the original ADR. Its codebase is dormant (last
  release Aug 2023), but as of 2026-09-03 the two nearest comparators link contributors
  straight at it: **LiteLLM** (`cla-assistant.io/BerriAI/litellm`), an AI gateway, and
  **Langfuse** (`cla-assistant.io/langfuse/langfuse`). The self-hosted
  `contributor-assistant/github-action` is archived (last release v2.6.1, Sep 2024).
- **Known defect, mitigated.** The service's CLA check can get stuck and never re-run after
  a contributor signs ([cla-assistant#520](https://github.com/cla-assistant/cla-assistant/issues/520), [#528](https://github.com/cla-assistant/cla-assistant/issues/528), both open as of 2026-09-03). Langfuse's published mitigations are adopted
  here: a `/check-cla` retrigger, and documentation of the author-email trap.

## Scope split

This issue is **not fully automatable**, and the AC reads as if it were. Two halves:

| Half | Owner | Content |
|---|---|---|
| Service linking | **Sven, in a browser** | Authorize cla-assistant.io's OAuth app, link `paigasus-core`, point it at the CLA document, set the allowlist |
| Repo artifacts | This PR | The CLA text, the retrigger workflow, the CONTRIBUTING rewrite |

The PR cannot verify the first half. AC-4 ("CLA flow verified end-to-end") is therefore
satisfied by its own documented-path fallback, not by a live test — see Verification.

## Design

### D1 — `docs/CLA.md`, an adapted Apache ICLA

The CLA text lives **in the repo**, not only in a gist. Version-controlling it means the exact
text a contributor agreed to on a given date is recoverable from git history. Under the hosted service the *signature records* live in
SAP's database; the *agreement text* must not also live somewhere we do not control.

**Premise to verify before writing this file.** D1 assumes cla-assistant.io accepts a
**repository file URL** as the CLA document. The hosted linking flow may be gist-only. If it
is, the authoritative text becomes a mutable gist in a personal account and `docs/CLA.md`
becomes an ungated mirror — a two-artifact drift problem in a repo that gates codegen drift,
version lockstep and 26 napi version guards. **Contingency:** if gist-only, `docs/CLA.md` must
carry a pointer to the gist revision it mirrors, plus either a drift assertion or a written,
reasoned waiver. Do not write D1 before checking this in the browser.

Source: the Apache Individual Contributor License Agreement, clauses 1–8, adapted. **Three
substantive changes**, not two structural ones — an earlier draft of this spec understated it:

1. **"the Foundation" → a named natural person: Sven Maschek, personally.** Apache's template
   names the ASF throughout. Decided on this issue.

   **No postal address, and that is consistent with the template rather than a shortcut.**
   MEASURED against primary sources: the ASF ICLA v2.2 identifies its recipient as "The Apache
   Software Foundation (the 'Foundation')" and carries no recipient address anywhere; its
   `Residence Address` field belongs to the CONTRIBUTOR (and we capture GitHub login only, so we
   do not collect it either). Grafana's adapted CLA does the same — "Raintank, Inc. dba Grafana
   Labs" — as do Google's, GitLab's and OASIS's individual CLAs. The requirement is an
   identifiable counterparty, not an addressable one.

   A personal name is not inherently unique the way a company name is, so identification is by
   name plus a stable public identifier: **"Sven Maschek, maintainer of the Paigasus project
   (GitHub: `SMK1085`)"**. Publicly verifiable, discloses nothing not already public.

   **Not resolved here, and out of this spec's competence:** German Impressum duties (DDG/TMG §5)
   can require a physical address for certain commercial online offerings, and whether a public
   repository triggers them is contested. That is a site-disclosure question rather than a CLA
   question, so it does not change this design; if it ever applies, a business address satisfies
   it, as would a future GmbH/UG's Handelsregister address.
   Because the counterparty is a natural person rather than the eventual operating company,
   change 2 below stops being boilerplate and becomes the clause the whole arrangement rests on.
2. **A successors-and-assigns clause, which the ASF ICLA does not contain. Load-bearing here.**
   With Sven as the personal counterparty, every contribution is licensed to an individual, and
   the expected end state is a GmbH/UG operating `paigasus-cloud`. Without an assignment right,
   moving those licenses to that company later needs each contributor's consent — the exact
   retroactive-permission problem ADR-0007 exists to avoid, reintroduced through the back door.
   A non-exclusive
   copyright license is presumptively not assignable without the licensor's consent (*Gardner
   v. Nike*, 9th Cir. 2002, and comparable reasoning elsewhere). Clause 2's sublicense right
   mitigates but does not cleanly substitute for transferring the head license. Since
   ADR-0007's whole purpose is optionality — including moving code into `paigasus-cloud`,
   presumably under a company — a CLA that cannot follow the business to that company defeats
   itself. Add an explicit right to assign to a successor entity or asset acquirer.
3. **Wet-signature block → click-through acceptance.** The ICLA ends with
   `Please sign: ______ Date: ______`, written for a fax/scan flow. That is replaced by a
   statement that signing via cla-assistant.io constitutes acceptance.

**Versioning is mandatory, not optional.** The "recoverable from git history" property above is
only true if a signature pins a version. `docs/CLA.md` carries `Version:` and `Effective:`
headers; any substantive edit bumps the version and requires re-signature. Without this, editing
the file silently converts existing signatures into signatures against text nobody agreed to.

The operative grants are **not** reworded. This matters legally and is the whole premise of
ADR-0007: clause 2 grants a perpetual, irrevocable copyright license "to reproduce, prepare
derivative works of, publicly display, publicly perform, **sublicense**, and distribute Your
Contributions". The sublicense right is precisely what permits relicensing, dual-licensing,
and use in `paigasus-cloud`. Clause 3 carries the patent grant with defensive termination.
Rewriting these to be "simpler" would silently forfeit what the CLA is for.

**No SPDX header.** Every source file in this repo opens with one, but `docs/CLA.md` is a
legal instrument, not licensed source; stamping `Apache-2.0` on the agreement that governs
contributions *into* an Apache-2.0 project is a category error and would confuse the reader
about which document governs what. `docs/` markdown carries no SPDX header today either
(`docs/dev-setup.md`, `docs/ops/RUNBOOK-*.md`), so this follows the existing convention rather
than inventing an exception.

**This is a legal document and I am not a lawyer.** The adaptation is faithful to a widely
used public template, but the decision to adopt it is Sven's, and it is the one artifact here
worth a second read before merge.

### D2 — `.github/workflows/cla-retrigger.yml`

Modeled on Langfuse's published workaround. Commenting `/check-cla` on a PR re-runs the
stuck check.

**The request contract is pinned here, because a vague `curl` is a control that lies.** A
`curl` that 404s or reaches a down service still exits 0, the job goes green, and CONTRIBUTING
has told the contributor the check will re-run when nothing happened. That is the same failure
shape `ci/release-parity`'s five-line pin and `ci/cargo-lock-integrity`'s three-mode invocation
exist to prevent.

- **Endpoint:** `GET https://cla-assistant.io/check/SMK1085/paigasus-core?pullRequest=<n>`
  (the form Langfuse uses, with our owner/repo). **Verify this responds as expected during the
  browser half before relying on it.**
- **Invocation:** `curl --fail-with-body -sS`, so a non-2xx is a non-zero exit and the job goes
  red rather than green-and-silent.
- **Trigger:** `issue_comment: types: [created]` only. Never `pull_request_target`.
- **Permissions:** `permissions: {}`. The endpoint is unauthenticated; the job needs no token.
- **Guard:** `if: github.event.issue.pull_request && startsWith(trim(github.event.comment.body), '/check-cla')`.
  `startsWith(trim(...))` rather than `contains()`, because `contains()` self-triggers on any
  comment that merely quotes the CONTRIBUTING instruction.
- **`concurrency: { group: cla-retrigger-${{ github.event.issue.number }}, cancel-in-progress: true }`
  and `timeout-minutes: 5`.** Any GitHub account can comment on a public repo, so this trigger is
  world-reachable. Actions minutes are free on public repos, but the account-level concurrent-job
  limit is shared with `ci.yml`, so an uncapped comment flood is a denial-of-CI on the repo's
  required checks. Every other workflow here carries both keys.

**Known limitation, documented rather than fixed:** an `issue_comment` run attaches no check to
the PR, so it surfaces only in the Actions tab on `main`. A contributor who comments `/check-cla`
gets no direct signal. Adding a reaction or reply would need `issues: write`, which reintroduces
a write scope on a world-triggerable workflow — not worth it. D3 documents the silence instead.

**This workflow cannot be exercised on its own PR.** `issue_comment` workflows run from the file
as it exists on the default branch, so the PR that adds it can never fire it. A post-merge smoke
is therefore an acceptance criterion, not an optional nicety.

### D3 — `CONTRIBUTING.md` rewrite

Replace the three-line placeholder at `:212-217` with the live flow plus **both** documented
traps. The traps are not optional polish: each one produces a check that stays red after the
contributor has done everything right, which is the single worst first-contribution experience
a project can offer.

1. The signing link, and that it is once-per-contributor.
2. **Stuck check** → comment `/check-cla` to retrigger, and that the retrigger reports back
   only in the Actions tab (see D2's known limitation): wait, refresh the checks list, and open
   an issue if it stays red.
3. **Author-email trap** → a commit whose author header omits the contributor's GitHub email
   will not clear the check even after signing; fix with `git config user.email` and a rebase.
   This repo's own history already contains one such commit
   (`Sven Maschek <smaschek@outlook.com>`), so the trap is real here, not theoretical.
4. **A link to `docs/CLA.md`**, which nothing else in the repo would otherwise reference.
5. **Reconcile the enforcement wording.** The current text at `:216-217` says external
   contributions "can't be merged without it". With an advisory check that is false, and
   shipping it unchanged would make the repo's own contributor documentation inaccurate.
6. **A privacy note** naming cla-assistant.io / SAP as the processor of signature data.

### D4 — extend `repo:workflow-credentials` to cover `issue_comment`

**This is new scope, added in response to the adversarial review, and it is the finding I got
wrong.** The first draft of this spec listed "stays outside the gate's subject set" in the
Gate-analysis table and called it "the main reason the hosted mechanism is cheap here" — i.e.
it presented a coverage gap as a design benefit.

`issue_comment` runs in **base-repo context with full access to repository secrets**, and on a
public repo **any GitHub account can trigger it**. That is the same privileged class the gate's
own README already reasons about: `ci/workflow-credentials/README.md` Non-goals names
`workflow_run` and `merge_group` as secrets-bearing triggers outside `PR_TRIGGERS`, notes
"Neither is used in this repository today", and prices the fix — *"Adding either is a one-line
change to the trigger set plus two new control rows, not a redesign."* `issue_comment` belongs
in that list and is not in it.

`permissions: {}` is correct in the file we are about to write. Nothing would notice a later
edit adding `permissions: contents: write` or a `${{ secrets.X }}` read to it.

Since this issue introduces the repo's **first** `issue_comment` workflow, bringing the class
under the gate is proportionate rather than scope creep — we are covering what we are adding.

- Add `issue_comment` to `PR_TRIGGERS` (`workflow_credentials.py`, currently `:278`).
- Re-baseline `EXPECTED_PR_SUBJECTS` (`:284-290`) from five entries to six, adding
  `cla-retrigger.yml` in sorted position.
- Add the corresponding `TRIGGER_CASES` control rows and update the `rows +=` self-test count.
- Update the README's Non-goals so the list stays honest about what remains uncovered.

The new workflow then passes R1–R5 on its own merits — no `secrets` key, no `secrets` context
read, no write scope — rather than by not being looked at.

## Gate analysis

The retrigger workflow touches **no gate registry**, by construction:

| Gate | Effect | Why |
|---|---|---|
| `repo:workflow-credentials` | **modified by D4**; then a subject that passes | Before D4: the file is still *scheduled and parsed* — `discover()` calls `load_documents()` on every matched file and raises on malformed YAML, a non-mapping top level, or duplicate keys *before* any subject filtering, and the gate's `inputs` include `.github/workflows/*.y*ml`. So "no effect" was wrong even under the old design. After D4 it is a full subject and passes R1–R5 on its merits. |
| `repo:actionlint` | passes | The workflow declares no `branches:`/`paths:` filters at all. `types:` is **not** among the extractor's four recognised filter keys (`paths`, `paths-ignore`, `branches`, `branches-ignore`), so the inline `types: [created]` form never reaches check 5/6 and the CLAUDE.md block-sequence rule does not apply to it — if a `branches:` filter is ever added, it must be a block sequence. shellcheck runs over the `run:` block, but actionlint replaces `${{ }}` with inert placeholders, so the `github.event.issue.number` interpolation is not covered; it is a GitHub-supplied integer, not attacker-controlled text. |
| `repo:affected-smoke` | unchanged | No new `repo:*` task, so none of the seven registration obligations apply. |
| `repo:input-liveness` | unchanged | No task `inputs` added or moved. |

The hosted mechanism is still much cheaper than the self-hosted one — the archived action needs
`pull_request_target` plus four write scopes plus a `secrets` read, which would have meant an
`EXPECTED_PR_SUBJECTS` re-baseline *and* two `PR_CREDENTIAL_ALLOWED` entries permitting a real
credential. Ours needs the re-baseline and permits nothing. But "cheap" now means "declares no
credential under a gate that looks", not "is not looked at".

## Allowlist

**cla-assistant evaluates the authors of the PR's commits, not the account that opened the PR.**
An earlier draft derived this list from `gh pr list --json author` — the wrong population.
Langfuse's documented author-email trap confirms the commit-author semantics. Re-derived from
`git log --format='%an <%ae>'` over the last 300 commits on `main`, cross-checked against
GitHub's commits API for whether each address resolves to a login:

| Commit author | Commits | Resolves to | Allowlist |
|---|---|---|---|
| `Sven Maschek <SMK1085@users.noreply.github.com>` | 149 | `SMK1085` | yes — owner |
| `dependabot[bot] <49699333+dependabot[bot]@…>` | 48 | `dependabot[bot]` | yes |
| `paigasusbot[bot] <285361405+paigasusbot[bot]@…>` | 2 | `paigasusbot[bot]` | yes — release PRs |
| `Sven Maschek <smaschek@outlook.com>` | 1 | **null** | n/a — see below |

Two findings from doing this properly:

**A latent defect in `release.yml`.** `.github/workflows/release.yml:384-385` configures the
version-lockstep stamping commit as `paigasus-release[bot]` /
`paigasus-release[bot]@users.noreply.github.com`. That address lacks the `<id>+<login>@` form
every resolvable bot identity above uses, so GitHub returns `author: null` for such a commit and
the allowlist cannot name it. It has produced **zero** commits to date — the step is conditional
on there being something to stamp — so nothing is broken today. But the first time it fires on a
release PR, that PR gets an unresolvable commit author and the CLA check blocks it. **Fix
`release.yml` to use the resolvable form** rather than allowlisting an unresolvable identity.

**The author-email trap is already in this repo's history.** One commit on `main` is authored
`Sven Maschek <smaschek@outlook.com>`, which resolves to no login. It predates the CLA and is
harmless, but it is direct evidence that D3's trap documentation is warranted.

**Unverified:** whether cla-assistant's allowlist field accepts bracketed logins literally
(`dependabot[bot]`), a glob (`dependabot*`), or uses a separate bot-exclusion setting. Confirm
during the browser half and record the exact string entered.

## Verification

- **AC-1/2/3** are verified by inspection of the linked service configuration after the browser
  half. They cannot be asserted from the repo.
- **AC-4** — a live end-to-end test needs a second GitHub account and a throwaway PR. This spec
  does **not** silently downgrade it to "documented path": that converts an acceptance criterion
  into an intention. It is filed as an explicit follow-up (see Open questions) so the
  renegotiation is on the record in Linear rather than buried here.
- **Post-merge smoke, mandatory.** `issue_comment` workflows run from the default-branch copy, so
  D2 cannot be exercised on its own PR. After merge, comment `/check-cla` on a live PR, confirm
  the run appears and exits 0, and record the run URL on the issue.
- `moon ci` over the affected graph, per the repo's normal pre-push rule. Note D4 changes a gate,
  so `repo:workflow-credentials`' own `--self-test` and `--negative-control` must pass.

**Honest limit:** nothing in CI asserts that the CLA service stays linked or keeps working. If
someone revokes the OAuth authorization, no gate reds — the check simply stops appearing. This is
inherent to a hosted mechanism with no in-repo footprint, and it is the cost the ADR accepted. It
is recorded here rather than left to be rediscovered.

## Residual risks

1. **Records live in SAP's database.** The signature records are the durable legal asset and we
   do not hold them. ADR-0007 Amendment 1 records the mitigation as a revisit trigger: arrange
   signature export before it becomes urgent.
2. **Personal-data processing with no notice.** Signing submits name, GitHub login, email, IP and
   timestamp to a third party, for a controller in the EU. There is no privacy notice, no stated
   lawful basis, no named processor relationship and no retention position today. D3 adds a short
   notice; confirming SAP's DPA / privacy terms is part of the browser half. This is a compliance
   and contributor-trust exposure, not merely a legal-custody one.
3. **A required check can deadlock the repo, and the ordering of the browser half prevents it.**
   Decided on this issue: the cla-assistant status **is** added to the `Protect main` ruleset's
   required-checks list, on the grounds that the ruleset carries an admin `bypass_actors` entry,
   so a stuck check is escapable by the one person who needs to escape it. Two constraints follow
   and they are not optional:

   - **The exact check context name is unknown until the service posts its first status.** You
     cannot add a required check by guessing its name; a wrong name blocks every PR forever
     while waiting for a context that never reports. Link the service, open a throwaway PR,
     read the real context name, *then* add it to the ruleset.
   - **Unverified and blocking: does cla-assistant post a passing status for an allowlisted
     author, or no status at all?** If it stays silent for allowlisted accounts, then making it
     required blocks every PR by Sven, `dependabot[bot]` and `paigasusbot[bot]` — i.e. all
     traffic this repo actually has. **Verify this on a real allowlisted PR before adding the
     required check**, not after. The admin bypass makes this recoverable, not harmless: every
     Dependabot PR would need manual bypass until it was undone.

   With those two ordered correctly, the deliverable does deliver ADR-0007's requirement that
   the CLA be in place before an external contribution is accepted, and the control is
   automation rather than maintainer discipline.
4. **The service may be sunset.** Dormant codebase; the migration path (an in-house action on the
   n8n model) is recorded in the ADR.

## Decisions taken on this issue

All four questions raised by the adversarial review are now settled:

1. **Counterparty: Sven Maschek, personally**, identified as "Sven Maschek, maintainer of the
   Paigasus project (GitHub: `SMK1085`)". Not a company, and **no postal address** — the ASF
   ICLA and every comparable adapted CLA name their recipient without one; see D1 for the
   measurement. This makes D1's successors-and-assigns clause load-bearing rather than
   boilerplate. Nothing further is needed before `docs/CLA.md` can be written.
2. **Identity captured: GitHub login only** — account, email, timestamp, as cla-assistant
   captures by default. No custom name/address fields. Accepted trade-off: a pseudonymous
   account is harder to bind to a real person, so enforceability is weaker than the ASF
   signature block contemplates. Judged proportionate for a project of this size, and it is
   what LiteLLM and Langfuse accept. It also keeps the GDPR surface (residual risk 2) to the
   minimum the mechanism requires.
3. **AC-4: filed as a linked follow-up issue** rather than downgraded in place, so the
   renegotiation is on the record in Linear.
4. **Employer IP: ICLA clause 4's representation only.** The contributor represents they have
   employer permission or that the employer waived its rights. No CCLA now; a corporate
   contributor population does not exist yet, and a second document plus a second signing flow
   is cost without a payer.

## Out of scope

- A Corporate CLA (CCLA); see decision 4.
- Migrating to a self-hosted action (ADR-0007 Amendment 1 records when to revisit).
- Signature-export automation.
- Fixing `release.yml:384-385`'s identity — identified here, but it is a separate defect with a
  separate blast radius and should not ride along on a CLA PR.

## Acceptance criteria

- [ ] `docs/CLA.md` exists: adapted Apache ICLA, operative grants unmodified, counterparty named
      as "Sven Maschek, maintainer of the Paigasus project (GitHub: `SMK1085`)" with no postal
      address, successors-and-assigns clause, `Version:` and `Effective:` headers, ICLA clause 4
      employer representation retained
- [ ] `.github/workflows/cla-retrigger.yml` exists: `issue_comment` trigger, `permissions: {}`,
      pinned endpoint with `curl --fail-with-body -sS`, `startsWith(trim(...))` guard,
      `concurrency` group and `timeout-minutes`
- [ ] D4 applied: `issue_comment` in `PR_TRIGGERS`, `EXPECTED_PR_SUBJECTS` re-baselined to six,
      control rows and self-test count updated, README Non-goals corrected
- [ ] `CONTRIBUTING.md` CLA section: live flow, both traps, retrigger-silence note, link to
      `docs/CLA.md`, corrected enforcement wording, privacy note
- [ ] Allowlist recorded as `SMK1085`, `dependabot[bot]`, `paigasusbot[bot]`
- [ ] Verified that cla-assistant posts a passing status for an allowlisted author, THEN the
      real context name added to the `Protect main` ruleset as a required check
- [ ] `moon ci` green over the affected graph, including `repo:workflow-credentials`'
      self-test and negative control
- [ ] Handoff note in the PR description stating exactly what Sven must do in the browser
- [ ] Post-merge: `/check-cla` smoke run recorded on the issue

## Files touched

| File | Change |
|---|---|
| `docs/CLA.md` | new |
| `.github/workflows/cla-retrigger.yml` | new |
| `ci/workflow-credentials/workflow_credentials.py` | D4: trigger set, subject pin, control rows |
| `ci/workflow-credentials/README.md` | D4: Non-goals corrected |
| `CONTRIBUTING.md` | rewrite `:212-217` |
| `docs/superpowers/specs/2026-09-03-sma-408-cla-bot-design.md` | this spec |
