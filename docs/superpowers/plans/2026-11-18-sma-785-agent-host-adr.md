# SMA-785 ADR-0028 publication plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** executed on 2026-11-18. The Notion pages and the Linear update exist. Do not run
Tasks 1 to 4 again: that would make duplicate pages.

**Goal:** Publish the approved agent-host decisions as ADR-0028 (Proposed), a Helikon decision entry
(Proposed) and a Linear project description update, and record the spec in the repo.

**Architecture:** Documentation only. No code. The spec is the source text; the Notion pages are
renderings of it.

**Tech Stack:** Notion MCP, Linear MCP, `git`, `gh`.

**Spec:** `docs/superpowers/specs/2026-11-18-sma-785-agent-host-adr-design.md`

## Global Constraints

- ADR title: `ADR-0028: Always-on agent host — Postgres durability, inbox lease, CloudEvents envelope`.
- ADR status: `Proposed`. Date: `2026-11-18`.
- ADR parent page: "Architecture Decision Records", id `368830e8-fbaa-816c-b411-c7ee1682c175`.
- Helikon decision database data source: `collection://7078dae0-4eda-49c1-91df-2ef721ae2ec6`.
- Linear project: "Paigasus Agent Host" (`P-SMA-8`).
- All text in ASD-STE100 Simplified Technical English. Keep technical names unchanged.
- Do not change any Accepted ADR. Do not change any other Notion page except the ADR index table.

## Review Focus

- The Notion page must match the approved spec, decision for decision: (a) to (o), with no
  dropped caveat. Check: count the lettered decisions on the page (15).
- The index row must be a page mention, like the existing rows, in number order after ADR-0027.
- The Helikon entry must not claim the changes are accepted or scheduled. Status is Proposed.
- The Linear patch must add lines, not rewrite the existing description.
- The commit must pass the commit-msg hook (commitlint) without `--no-verify`.

---

### Task 1: ADR-0028 page in Notion

**Files:** none in the repo. Notion: a new child page of `368830e8-fbaa-816c-b411-c7ee1682c175`.

**Interfaces:**
- Produces: the ADR-0028 page URL, used by Tasks 2, 3, 4 and 5.

- [ ] **Step 1:** Fetch ADR-0027 and copy its layout: bold metadata lines, then `## Context`,
  `## Decision`, `## Consequences`, `## Alternatives considered`, `## Revisit triggers`,
  `## References`.
- [ ] **Step 2:** Create the page. Metadata: `Status: Proposed`, `Date: 2026-11-18`,
  `Decided by: Sven Maschek`, `Issue:` SMA-785 Linear URL, `Spec:` the spec path,
  `Builds on: ADR-0005, ADR-0006, ADR-0014, ADR-0016`. Body: spec sections 3 to 7, with
  decisions (a) to (o) as `###` headings. Consequences use bold `Positive` and
  `Negative and risks`. References: the Anthropic article, Helikon ADR-6, the spec path.
- [ ] **Step 3:** Fetch the new page. Expected: 15 decision headings, status Proposed.

### Task 2: Index row

**Files:** Notion page `368830e8-fbaa-816c-b411-c7ee1682c175`.

- [ ] **Step 1:** Fetch the parent page and read the index table format.
- [ ] **Step 2:** Add the row `0028 | <page mention> | Proposed | 2026-11-18` after ADR-0027.
- [ ] **Step 3:** Fetch again. Expected: the row is present and the other 27 rows are unchanged.

### Task 3: Helikon decision entry

**Files:** Notion data source `collection://7078dae0-4eda-49c1-91df-2ef721ae2ec6`.

- [ ] **Step 1:** Read the data source schema and one existing entry.
- [ ] **Step 2:** Create an entry. Title: `Deferred tool calls and a turn-boundary checkpoint`.
  Status `Proposed`. Date `2026-11-18`. Area: `Permissions & Guardrails`, `Sessions`, `Runtime`.
  Body: spec decision (g) "Helikon changes", decision (h) "Helikon change", and the
  `sessions-testkit` publication from (b), with a link to ADR-0028 as the driver.
- [ ] **Step 3:** Fetch the entry. Expected: status Proposed, link to ADR-0028 present.

### Task 4: Linear project description

**Files:** Linear project `P-SMA-8`.

- [ ] **Step 1:** Patch the `## Crates` section. Add: the CloudEvents library crate (from (j)),
  the Helikon dependency at a version with (g) and (h), and a line that links ADR-0028.
- [ ] **Step 2:** Fetch the project. Expected: the old lines are unchanged and the new lines are present.

### Task 5: Commit, push and PR

**Files:** `docs/superpowers/specs/2026-11-18-sma-785-agent-host-adr-design.md`,
`docs/superpowers/plans/2026-11-18-sma-785-agent-host-adr.md`.

- [ ] **Step 1:** Provision the worktree for the commit hook: `proto install`, then `pnpm -C ts install`.
- [ ] **Step 2:** Add the ADR-0028 URL to the spec header.
- [ ] **Step 3:** Commit: `docs(repo): record ADR-0028 for the always-on agent host (SMA-785)`.
- [ ] **Step 4:** Push and open the PR with `gh pr create`. Expected: the PR URL.
