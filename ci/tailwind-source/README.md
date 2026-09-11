# `repo` gate: Tailwind `@source` reachability (SMA-503 AC 3)

Asserts that a production `next build` of `@paigasus/iam-console` still emits the CSS that
`@paigasus/ui` contributes. Two independent sentinels:

| Sentinel | Declared in | Proves |
|---|---|---|
| `--paigasus-ui-source-probe` | `ts/packages/paigasus-ui/src/components/table.tsx` | Tailwind SCANNED the package's source, i.e. the app's `@source` line still covers it |
| `--paigasus-token-probe` | `ts/packages/paigasus-ui/src/styles/tokens.css` | the app's `@import '@paigasus/ui/styles.css'` RESOLVED, i.e. the token layer reached the output |

Sentinel A alone is not enough: the `@import` can fail, the whole token layer can be absent,
and sentinel A still passes.

## Why this script lives at the repository root

**It must never move under `ts/apps/iam-console/`.** Tailwind's automatic scan root is
the current working directory, and Moon runs `next build` from the console's own directory.
Tailwind extracts class candidates from any non-ignored text file. A script placed there and
containing the literal `[--paigasus-ui-source-probe:1]` would make Tailwind generate that
utility **from the script itself** — so the assertion would pass with the `@source` line
deleted. Excluding the script from its own "appears nowhere in the app" check reopens the
same hole from the other side.

## Invocation

Run by `iam-console-ts:test`, which depends on `~:build`. Three modes, in order:

- `--self-test` — drives the verdict function over synthetic fixtures in a temporary
  directory, proving the assertions can both pass and fail.
- `--negative-control` — asserts the script reports red against CSS lacking the probes.
- no flag — the real run, against `ts/apps/iam-console/.next`.

## Limitations

- **Nothing pins these three invocation lines.** `ci/affected-graph/ci_targets.py`'s
  `check_self_scheduled_coverage` scans `repo:*` tasks only, and this runs under
  `iam-console-ts:test`. Deleting the `--negative-control` line reds nothing. The
  alternative is a new `repo:*` gate running a full `next build` on every affected pull
  request, which was judged too expensive.
- **Coverage is per-consumer.** A green here says nothing about a second zone app. Every new
  app needs its own `@source` line and its own assertion.
- The script proves the CSS was EMITTED. It does not prove the page references it.
- **A cache-hit build can leave a stale CSS chunk that satisfies the sentinels.** `rm -rf
  .next/static` lives inside `iam-console-ts:build`'s own `script:`, so it runs only when
  Moon actually EXECUTES that task. On a cache hit Moon hydrates `outputs: ['.next']` from its
  tarball instead, the `rm` never runs, and the guard's fallback walk then reads whatever CSS
  chunks that hydrated tree contains. A chunk from an earlier build can carry both sentinels
  and green the assertion on its own.

  Evidenced on this branch rather than reasoned about: Proof 3 in
  `docs/superpowers/specs/2026-09-08-sma-503-measurements.md` reports *"both sentinels present
  across 2 CSS file(s)"* on a cache-hit run, while M4b and M6 in that same file each state that
  a live `next build` writes exactly ONE CSS file. The second file in that run was a stale
  chunk.

  This is NOT fixed. Fixing it means moving the removal out of the cached task — a structural
  change to how the build task and the guard are ordered — and that was out of scope for the
  fix round that recorded this. Two things bound the exposure meanwhile: a change to
  `@paigasus/ui`'s sources re-keys `build`, so the run that MATTERS (the one the input in
  §11.3 exists for) is a cache miss and does run the `rm`; and the manifest resolver would take
  precedence over the walk if Turbopack ever started naming CSS in
  `app-build-manifest.json`, which M6 measured it does not.
