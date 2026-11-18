# SMA-783: Console shell design pass

Status: design approved by Sven on 2026-11-17 and 2026-11-18 (design conversations, reviewed in
the Paigasus design system). It changes the layout SMA-510 § 8.1 specified.

## 1. Intent

The console shell from SMA-510 put the Primary nav in the header, mixed the current console's
pages with links to other consoles, and spent header space on full names. This pass makes the
shell read like a focused admin tool: one console at a time, a grouped sidebar, a console
switcher on the brand, and a compact header. It also closes the two contrast gaps the token layer
had.

## 2. Goals and non-goals

Goals:

- G1. The Primary nav is a vertical sidebar beside `main` that lists the current console's pages
  only, in groups.
- G2. Another console is reached through the brand, which becomes a console switcher.
- G3. The sidebar collapses at every width and starts collapsed on narrow screens.
- G4. The header fits on one line down to 320px: initials for the user, an initials badge for the
  organization.
- G5. Every text pair meets 4.5:1 and every focus ring 3:1, in both themes.

Non-goals:

- Remembering the sidebar choice across full page loads. It would need a cookie read on the
  server to render without a flash. The choice lasts for the page's life.
- Icons for nav entries, or a collapsed icon rail. Entries have no icons; collapsed means hidden.
- A team switcher. `initials` and `icon` are ready for one.

## 3. Constraints

- C1. No new runtime dependency and no lockfile change. Glyphs are Lucide (ISC) paths drawn
  inline, like `Dialog`'s close button. `lucide-react` stays out.
- C2. The accessible names that tests and the e2e suites use stay as they were: the user menu is
  still named by the display name, the org switcher by `Organization: <name>`.
- C3. No hydration mismatch and no flash: the server cannot know the viewport.
- C4. Hover never opens a menu. A pointer crossing the header must not pop it open, and touch has
  no hover.

## 4. Design

### 4.1 Shared entry rules (`src/nav/rows.ts`)

`PrimaryNav`'s entry rules (SMA-510 § 7.2) move to `navRows()`, so the console switcher applies
the same ones. `NavEntry` moves there and gains `group?: string`. A new rule 2a, on only for
`PrimaryNav`: an entry for another configured zone throws `ZoneLinkError`. It sits after rule 2 so
a structural error is loud even for an absent entry.

### 4.2 `PrimaryNav`

A `w-56` column with a right border. Rows are `rounded-pgs`, muted text, a muted fill on hover;
the current row keeps the fill, `pgs-foreground` and weight 500. Groups appear in the order of
their first entry. A group's entries are gathered even when not adjacent. Each heading names its
list through `aria-labelledby`. Ungrouped entries form an untitled list. It still returns null
with no visible entry.

### 4.3 Console switcher (`src/shell/console-switcher.tsx`)

`AppShell` takes `consoles?: readonly NavEntry[]`, every console including the current one. With
two or more left after `navRows()` (no rule 2a), the brand is a `DropdownMenu` trigger named
"Switch console: Paigasus <label>". The visible label stays inside the name (WCAG 2.5.3). On
hover, `focus-visible` and while open, the mark scales and fades into Lucide's `layout-grid`
(200ms, off under `prefers-reduced-motion`). The menu lists the consoles; the current one has a
tick and `aria-current="true"`. A degraded console is a disabled item with its reason, at full
opacity so the reason stays readable. With fewer than two consoles it is `BrandLink`.

The consoles build the list with `buildConsoleEntries()` in each app's `lib/nav.ts`, the same
order (IAM, AI Gateway) in both. It lives per app, mirroring `buildNavEntries()`, because
`@paigasus/console-core` does not depend on `@paigasus/app-shell` (C1). IAM's nav groups:
Directory (Organizations), Administration (Audit, Dead letters).

### 4.4 Collapsible sidebar (`src/shell/sidebar.ts`)

`useSidebar()` reads `(min-width: 48rem)` through `useSyncExternalStore`, with `true` as the
server snapshot, and keeps the user's choice (`open`, `closed` or null). Before a choice the
wrapper's classes are `hidden md:flex`, so CSS alone decides and a narrow screen never flashes the
sidebar (C3). Open on a narrow screen, it overlays `main` (`absolute`, `z-40`, `shadow-lg`).
Escape inside it closes it and returns focus to the toggle; following one of its links closes it.
Both handlers are native listeners on the wrapper, which only delegates. The toggle is the first
item in the header: a button named "Navigation" with `aria-expanded` and `aria-controls`, present
only when the nav has a visible entry.

### 4.5 Header compaction

- `UserMenu`: a 32px round avatar with `initialsOf(displayName, email)` (`src/shell/initials.ts`):
  first graphemes of the first and last words, else of the email's local part, upper-cased; a
  Lucide `user` glyph when neither exists. The full name is screen-reader text and the `title`.
- `Switcher`: `icon?: ReactNode` replaces the visible "Label:" prefix, and `initials?: boolean`
  leads with a 20px square badge (`radius-xs`, so it never reads as the round avatar). The name
  then truncates past 12rem and is `sr-only` below `md`. `title` carries `${label}: ${current}`.
  Initials alone were rejected for organizations: they collide, and the org switcher is what
  tells an IAM admin which tenant they are about to change. With nothing selected, `icon` and
  "none selected" show. `OrganizationIcon` is exported for this.
- The accessible name joins adjacent inline text without a separator and drops a trailing space
  inside an `sr-only` span, so `Switcher` puts a `{' '}` node after it, the same as `BrandLink`.

### 4.6 Tokens and `Dialog`

- `pgs-ring` light: `oklch(0.62 0.015 286.1)`, 3.6:1 on background and 3.3:1 on muted (was 2.6:1).
  Dark (4.1:1, 3.1:1 on muted) is unchanged.
- `pgs-destructive-foreground` dark: `oklch(0.21 0.006 285.9)`, 6.1:1 on `pgs-destructive` (white
  was 2.8:1). Light is unchanged at 4.6:1. Nothing uses the pair yet.
- `DialogFooter`: `flex flex-row items-center justify-end gap-2` at every width.

## 5. Tests

- `rows`/`PrimaryNav`: rule 2a throws, also for an absent entry; groups (order, gathering,
  untitled, a filtered-out group disappears). The fixtures' second entry moves into the current
  zone.
- `console-switcher.test.tsx`: name, decorative glyphs, list and tick, plain `<a>` across zones,
  keyboard open and Escape, degraded readable and disabled, absent hidden, `requires`, fallback.
- `sidebar.test.tsx`: wide and narrow defaults, toggle, narrow-only Escape and link close, no
  toggle without nav.
- `initials.test.ts`: words, case, whitespace, combining marks, non-Latin, email fallback.
- `switcher` and `user-menu`: icon, badge, truncation classes, tooltip, accessible names.
- `app-shell`: the keyboard script's tab order (toggle, brand, switcher, user menu, then the nav),
  the nav outside the banner, the console switcher in it.
- Both consoles: `buildNavEntries()` / `buildConsoleEntries()`, and the org switcher markup.
- e2e: the two cross-zone specs open the console switcher instead of clicking a nav link; the
  app-shell fixture's degraded entry moves into the current zone. Not run locally (Docker).

## 6. Left to the builder

- Contrast figures are computed from the OKLCH values (OKLab to linear sRGB, WCAG relative
  luminance), not measured on screen.
