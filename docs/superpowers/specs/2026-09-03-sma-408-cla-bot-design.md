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
  A survey of twelve comparable open-core projects found zero using a DCO.
- **Hosted cla-assistant.io**, unchanged from the original ADR. Its codebase is dormant,
  but the two nearest comparators — **LiteLLM** (an AI gateway) and **Langfuse** — both run
  it in production today. The self-hosted `contributor-assistant/github-action` is archived.
- **Known defect, mitigated.** The service's CLA check can get stuck and never re-run after
  a contributor signs (cla-assistant#520, #528). Langfuse's published mitigations are adopted
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

The CLA text lives **in the repo**, not only in a gist. cla-assistant accepts a repository
file URL, and version-controlling it means the exact text a contributor agreed to on a given
date is recoverable from git history. Under the hosted service the *signature records* live in
SAP's database; the *agreement text* must not also live somewhere we do not control.

Source: the Apache Individual Contributor License Agreement, clauses 1–8, adapted. Two
changes only, both structural rather than substantive:

1. **"the Foundation" → the project owner.** Apache's template names the ASF throughout;
   this names the Paigasus copyright holder.
2. **Wet-signature block → click-through acceptance.** The ICLA ends with
   `Please sign: ______ Date: ______`, written for a fax/scan flow. That is replaced by a
   statement that signing via cla-assistant.io constitutes acceptance, which is how every
   click-through adaptation of this document works.

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
stuck check via a plain `curl` to the service's check endpoint.

```yaml
on:
  issue_comment:
    types: [created]

permissions: {}
```

Three properties, all deliberate:

- **`issue_comment` only.** Not `pull_request_target`. This keeps the workflow out of
  `repo:workflow-credentials`'s subject set entirely — see Gate analysis.
- **`permissions: {}`.** The job needs no token at all; it calls a public endpoint.
- **Guarded on `github.event.issue.pull_request`**, so it ignores plain issue comments, and on
  the exact comment body, so arbitrary comments do not trigger runs.

### D3 — `CONTRIBUTING.md` rewrite

Replace the three-line placeholder at `:212-217` with the live flow plus **both** documented
traps. The traps are not optional polish: each one produces a check that stays red after the
contributor has done everything right, which is the single worst first-contribution experience
a project can offer.

1. The signing link, and that it is once-per-contributor.
2. **Stuck check** → comment `/check-cla` to retrigger.
3. **Author-email trap** → a commit whose author header omits the contributor's GitHub email
   will not clear the check even after signing; fix with `git config user.email` and a rebase.

## Gate analysis

The retrigger workflow touches **no gate registry**, by construction:

| Gate | Effect | Why |
|---|---|---|
| `repo:workflow-credentials` | none | `PR_TRIGGERS` is `{pull_request, pull_request_target}` (`workflow_credentials.py:278`). `issue_comment` is not a member, so the file never becomes a subject and `EXPECTED_PR_SUBJECTS` (`:284-290`, strict equality) is unchanged. R1–R5 have nothing to fire on: no `secrets` key, no `secrets` context read, no write scope, `permissions: {}`. |
| `repo:actionlint` | passes | Trigger filters are block sequences; shellcheck runs over the `run:` block. Note actionlint replaces `${{ }}` with inert placeholders, so the `github.event.issue.number` interpolation is *not* covered by shellcheck — it is an integer supplied by GitHub, not attacker-controlled text. |
| `repo:affected-smoke` | unchanged | No new `repo:*` task, so none of the seven registration obligations apply. |
| `repo:input-liveness` | unchanged | No task `inputs` added or moved. |

This is the main reason the hosted mechanism is cheap here and the self-hosted one was not:
the archived action needs `pull_request_target` plus four write scopes plus a `secrets` read,
which would have meant an `EXPECTED_PR_SUBJECTS` re-baseline and two `PR_CREDENTIAL_ALLOWED`
entries.

## Allowlist

Derived from the actual PR author population, not assumed. All 100 PRs to date:

| Account | PRs | Why allowlisted |
|---|---|---|
| `SMK1085` | 74 | Repository owner; ADR-0007's CLA governs *external* contribution |
| `dependabot[bot]` | 23 | Automated dependency PRs |
| `paigasusbot[bot]` | 3 | The GitHub App from SMA-589 that opens release-plz release PRs |

**`paigasusbot[bot]` is not in the issue's AC** and is the one that would actually break
something: without it, every release PR blocks on an unsigned CLA.

## Verification

- **AC-1/2/3** are verified by inspection of the linked service configuration after Sven
  completes the browser half. They cannot be asserted from the repo.
- **AC-4** is satisfied by the *documented path* — the AC's own stated alternative to a
  secondary-account test. A live end-to-end test needs a second GitHub account and a throwaway
  PR against a public repo; that is worth doing once, but it gates nothing here and should not
  hold the PR.
- **AC-5** is verified by the CONTRIBUTING diff.
- `moon ci` over the affected graph, per the repo's normal pre-push rule.

**Honest limit:** nothing in CI asserts that the CLA service stays linked or keeps working. If
someone revokes the OAuth authorization, no gate reds — the check simply stops appearing. This
is inherent to a hosted mechanism with no in-repo footprint, and it is the cost the ADR
accepted. It is recorded here rather than left to be rediscovered.

## Residual risks

1. **Records live in SAP's database.** The signature records are the durable legal asset and we
   do not hold them. ADR-0007 Amendment 1 records the mitigation as a revisit trigger: arrange
   signature export before it becomes urgent.
2. **The check is advisory, not required.** Per the decision taken on this issue, the
   cla-assistant status is *not* added to the `Protect main` ruleset's required-checks list
   yet, because the known stuck-check bug would then block legitimately-signed PRs. An unsigned
   PR can therefore be merged by an admin who ignores the status. Revisit once the flow has been
   exercised on a real external PR.
3. **The service may be sunset.** Dormant codebase; the migration path (an in-house action on
   the n8n model) is recorded in the ADR.

## Out of scope

- Making the CLA check a required status check (deliberate; risk 2 above).
- A Corporate CLA (CCLA). The individual agreement covers the expected contributor population;
  a CCLA can be added if a company contributes under an employer IP assignment.
- Migrating to a self-hosted action (ADR-0007 Amendment 1 records when to revisit).
- Signature-export automation.

## Acceptance criteria

- [ ] `docs/CLA.md` exists, adapted from the Apache ICLA with grants unmodified
- [ ] `.github/workflows/cla-retrigger.yml` exists: `issue_comment` trigger, `permissions: {}`,
      guarded on `github.event.issue.pull_request` and the exact comment body
- [ ] `CONTRIBUTING.md` CLA section documents the live flow plus both traps
- [ ] Allowlist documented as `SMK1085`, `dependabot[bot]`, `paigasusbot[bot]`
- [ ] `moon ci` green over the affected graph
- [ ] Handoff note stating exactly what Sven must do in the browser to finish

## Files touched

| File | Change |
|---|---|
| `docs/CLA.md` | new |
| `.github/workflows/cla-retrigger.yml` | new |
| `CONTRIBUTING.md` | rewrite `:212-217` |
| `docs/superpowers/specs/2026-09-03-sma-408-cla-bot-design.md` | this spec |
