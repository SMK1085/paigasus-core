# SMA-510 — `@paigasus/app-shell`: header, navigation and cross-zone linking

**Status:** approved design, revision 1 (before adversarial challenge)
**Date:** 2026-09-11
**Issue:** [SMA-510](https://linear.app/smaschek/issue/SMA-510/ts-paigasusapp-shell-header-navigation-and-cross-zone-linking)
**ADR:** ADR-0017 — Console topology & session ownership; ADR-0021 decision 3 (navigation is injected)
**Design:** Frontend Architecture Scoping § 6 (package table and dependency rules)
**Depends on:** SMA-503 (`@paigasus/ui`), SMA-506 (`@paigasus/auth`), SMA-508 (`@paigasus/sdk`), SMA-509 (`@paigasus/discovery`) — all Done
**Blocks:** SMA-511 (iam-console), SMA-512 (gateway-console)

---

## 1. Problem

The console is a set of Next.js apps, one per backend service, on one origin. The ingress
routes by path: `/iam/*` goes to the IAM zone and `/gateway/*` goes to the gateway zone. Each
app sets `basePath` for its own zone.

Every zone shows the same chrome: a header, primary navigation, an org/team switcher, a user
menu, and breadcrumbs. This package holds that chrome once.

The chrome is also the one place that must know the zone map. ADR-0017 states the hazard:

> "Cross-zone navigation is a hard navigation. A `<Link>` to another zone attempts a client-side
> RSC prefetch against a route its own server does not have, and fails. A `<ZoneLink>`
> abstraction is mandatory, and the failure mode — an intermittently broken nav item — is
> invisible to unit tests."

---

## 2. Scope

This issue delivers **package code only**, plus the repo gate changes that a new package
forces. It does not wire the shell into `paigasus-console`. SMA-511 does that ("Wrap in
`@paigasus/app-shell`").

**In scope:**

- The `@paigasus/app-shell` package: `ZoneProvider`, `ZoneLink`, `resolveZone`, `AppShell`,
  `PublicShell`, `PrimaryNav`, `navStateOf`, `Switcher`, `UserMenu`, `Breadcrumbs`.
- A jsdom test tier and a real-browser test tier (a package-local Next fixture app).
- One new client-safe export in `@paigasus/discovery` (§ 9.2).
- Boundary-rule, transpile-list, affected-graph and CI changes (§ 11).

**Out of scope:**

- Wiring the shell into any console app, and the app's Tailwind `@source` line for this
  package. Both belong to SMA-511.
- **The SMA-508 failing-build fixture.** SMA-508 AC 1 assigned "a console-side failing-build
  fixture" to this issue on the assumption that SMA-510 has a console that consumes the SDK. It
  does not. Also, `paigasus/boundaries/app-shell` bans `@paigasus/sdk` in every file under this
  package, tests included, so the fixture cannot live here. **Decision (Sven, 2026-09-11): the
  fixture moves to SMA-511**, the first console that consumes the SDK. A comment on SMA-508 and
  SMA-511 records the move.
- The multi-zone ingress and the cross-app E2E harness (SMA-513).
- Search in the switcher, breadcrumbs derived from routes, and any version or skew indicator.
  ADR-0020's amendment states that the console "must not render a skew banner driven by a
  constant", and `ServiceInfo.version` is `0.0.0` on every deployment today.

---

## 3. Facts established before design

Each fact has a source. Later sections depend on them.

| # | Fact | Source |
|---|---|---|
| F1 | The zone map already exists. `PAIGASUS_ZONES` is a JSON object, zone id → canonical base path. `getPublicConfig()` returns `{ zone, zones }` and is the allowlisted client-safe projection. It is `server-only`. | `ts/packages/paigasus-next-config/src/runtime.ts` |
| F2 | A zone may be root-mounted. `canonicalBasePath` maps `''` and `'/'` to `''`. | `ts/packages/paigasus-next-config/src/base-path.ts:23-25` |
| F3 | `next/link` adds the app's `basePath` to its href. A plain `<a>` does not. | Next.js `basePath` behaviour |
| F4 | `@paigasus/ui` defines the navigation contract: `LinkProps = { href; className?; children? }`, `LinkProvider`, `useLinkComponent`, `Link`. The context default is a plain `<a>`. The console injects raw `next/link` today (`app/providers.tsx`). | `ts/packages/paigasus-ui/src/nav/link.tsx` |
| F5 | `SessionView` is `{ principalPrn, displayName, email, grants, grantsAvailable }`. It carries **no memberships** and no current org or team. `toSessionView()` drops the server-side `memberships`. | `ts/packages/paigasus-auth/src/session-view.ts`, `src/core/session.ts` |
| F6 | `useSession()` throws `UseSessionOutsideProviderError` when no `SessionProvider` is above it. `SessionProvider` takes a non-null `SessionView`. | `ts/packages/paigasus-auth/src/client.ts` |
| F7 | `can(session, { scopePrn, roleKey })` is cosmetic and **fails open** when `grantsAvailable` is false. Its comment names SMA-510: failing closed "would render a console with NO navigation at all". | `ts/packages/paigasus-auth/src/client.ts:53-69` |
| F8 | Login is `GET ${basePath}/auth/login`. It rejects a request whose `Sec-Fetch-Mode` is present and is not `navigate`. Logout is `POST ${basePath}/auth/logout` with no CSRF token (`SameSite=Lax` withholds the cookie from a cross-site POST). Its response is a redirect to the IdP's end-session endpoint. | `ts/packages/paigasus-auth/src/http/routes.ts:57-76, 96-112` |
| F9 | Discovery's `./react` and `./server` entries are `server-only`. `<Capability>` is an async server component. `CapabilityDisabled` and `reasonText` live in `src/disabled.tsx` (`'use client'`), but today they are reachable only through `./react`. `./types` is client-safe on purpose. | `ts/packages/paigasus-discovery/src/react.tsx:2`, `tests/structure/exports.test.ts` |
| F10 | Discovery's README already fixes this package's contract: "App-shell exports navigation _presentation_ taking resolved state as props; the **app** composes `<Capability>` around it." | `ts/packages/paigasus-discovery/README.md:147-152` |
| F11 | `CapabilityDisabled` shows its reason only to assistive technology. The reason text is visually hidden. | `ts/packages/paigasus-discovery/src/disabled.tsx` |
| F12 | The boundary rule for this package already exists and is inert. It bans `@paigasus/sdk` and `@paigasus/auth/server`, value and type imports, in every file under `packages/paigasus-app-shell/`. `BOUNDARY_SCOPES` holds a "has not landed yet" placeholder for it. | `ts/packages/paigasus-next-config/src/eslint.mjs:51-58, 108-117` |
| F13 | `createNextConfig` transpiles a hand-maintained `SOURCE_ONLY_PACKAGES` list. It does not contain `@paigasus/auth` or `@paigasus/discovery`. | `ts/packages/paigasus-next-config/src/index.ts:12` |
| F14 | `@paigasus/ui` has no breadcrumbs, navigation-menu or tooltip component. It has `DropdownMenu*` (Radix) and `Combobox` (cmdk). | `ts/packages/paigasus-ui/src/index.ts` |
| F15 | `@paigasus/ui` runs axe through `axe-core` directly, with the helper `expectNoAxeViolations(target = document.body)`. jsdom axe cannot evaluate colour contrast. | `ts/packages/paigasus-ui/tests/axe.ts`, `README.md` "The axe limit" |
| F16 | No Next app in the repo runs a browser test today. The two `test-e2e` tasks (`auth`, `discovery`) run Playwright against a plain Node server or a Vite fixture. | `ts/packages/*/playwright.config.ts` |

---

## 4. Decisions taken with Sven during brainstorming

| # | Decision | Choice |
|---|---|---|
| D1 | Where the org/team switcher gets its data and keeps the selection. | **Props only; the URL owns the selection.** The app passes `items` and `currentId`. The app (SMA-511) decides where the selection lives. No change to `@paigasus/auth`. |
| D2 | Where the real-browser proof of AC 1 and AC 2 lives. | **A package-local Next fixture app**, built with `next build` and driven by Playwright in this package's `test-e2e` task. |
| D3 | The SMA-508 failing-build fixture. | **Moves to SMA-511** (§ 2). |
| D4 | The `<ZoneLink>` href contract. | **Full path, resolved by segment-aware longest-prefix match** (§ 6). |

---

## 5. Package shape

```
ts/packages/paigasus-app-shell/
  package.json          @paigasus/app-shell, private, type: module, source-only
  moon.yml              id paigasus-app-shell-ts, layer library
  tsconfig.json
  vitest.config.ts      jsdom, like @paigasus/ui
  playwright.config.ts
  README.md
  src/
    index.ts            the one public entry
    zone/resolve.ts     resolveZone, ZoneLinkError   (no 'use client')
    zone/context.tsx    ZoneProvider, useZone        ('use client')
    zone/zone-link.tsx  ZoneLink                     ('use client')
    nav/state.ts        NavEntryState, navStateOf    (no 'use client')
    nav/primary-nav.tsx PrimaryNav, NavEntry         ('use client')
    shell/app-shell.tsx AppShell                     ('use client')
    shell/public-shell.tsx PublicShell               ('use client')
    shell/switcher.tsx  Switcher                     ('use client')
    shell/user-menu.tsx UserMenu                     ('use client')
    shell/breadcrumbs.tsx Breadcrumbs                ('use client')
  tests/
    *.test.ts(x)        jsdom tier
    e2e/                Playwright specs, and fixture/ (a Next app)
```

The file list is indicative. The plan fixes the final layout.

### 5.1 Entry point

One export: `"."` → `./src/index.ts`. `@paigasus/auth` and `@paigasus/discovery` omit a root
export because they have a server surface that a client must not reach. This package has no
server surface. Every file in it is client-reachable, so one root entry is correct.

### 5.2 The `'use client'` split

Component files carry `'use client'`, because they read React context. Pure helpers
(`resolveZone`, `navStateOf`) live in files **without** the directive.

The reason: a function exported from a `'use client'` module becomes a client reference when a
server component imports it. The server component can pass it along, but it cannot call it. An
app's server layout must be able to call `navStateOf()` on the `ServiceState` it resolved.
`src/index.ts` re-exports both kinds and carries no directive itself.

### 5.3 Dependencies

| Package | Kind | Used for |
|---|---|---|
| `@paigasus/ui` | `workspace:*` | `DropdownMenu*`, `cn`, the `LinkComponent` type |
| `@paigasus/auth` | `workspace:*` | `/client` only: `useSession`, `can`, `SessionView`, `RoleGrantRef` types |
| `@paigasus/discovery` | `workspace:*` | `/types` (`ServiceState`, `DegradedReason`) and the new `/disabled` (`reasonText`) |
| `next`, `react`, `react-dom` | peer, `catalog:` | `next/link`, `next/navigation` |

Test-only: `@testing-library/*`, `jsdom`, `axe-core`, `@playwright/test`, `vitest`, `typescript`,
`@types/*`, and `next` (for the fixture build). All come from the existing catalog.

---

## 6. `ZoneProvider` and `ZoneLink`

### 6.1 `ZoneProvider`

```ts
type ZoneMap = Readonly<Record<string, string>>; // zone id -> canonical base path

function ZoneProvider(props: { zone: string; zones: ZoneMap; children: ReactNode }): ReactElement;
function useZone(): { zone: string; basePath: string; zones: ZoneMap };
```

- The app's server layout calls `getPublicConfig()` (F1) and passes `zone` and `zones`. No other
  value crosses to the browser. Internal service URLs never do.
- `ZoneMap` is a structural type that this package defines. The package does not import
  `PublicConfig`, because that type lives in a `server-only` module, and § 11.1 bans that module
  from this package (value and type imports).
- `ZoneProvider` throws `ZoneConfigError` in two cases: `zones[zone]` is undefined, or two zones
  have the same base path. Both are deployment errors. `@paigasus/next-config` validates each
  base path, but it does not check that the values are unique.
- `useZone()` outside a provider throws `ZoneProviderMissingError`. There is no default. A
  default zone map would make every cross-zone link look same-zone, which is the AC 1 bug.

### 6.2 The href contract

The caller writes the URL as the ingress sees it: `<ZoneLink href="/gateway/usage">`,
`<ZoneLink href="/iam/users?page=2#top">`.

The href must be a path that starts with exactly one `/`. These forms are programming errors, and
`resolveZone` throws `ZoneLinkError` for them:

- an absolute URL (`https://…`, `mailto:…`, any scheme);
- a protocol-relative URL (`//host/…`);
- a relative path (`users`, `./users`, `../x`);
- an empty string.

The error does not fall back to a plain `<a>`. A silent fallback would hide a wrong href until a
user clicks it. The error message does not echo the href, because an href can carry user data.

### 6.3 Resolution

`resolveZone(href, zones)` returns `{ zone, basePath, rest } | null`:

1. Split the href into pathname, and query plus fragment. Only the pathname takes part in the
   match.
2. A zone with base path `b` matches when `pathname === b` or `pathname` starts with `b + '/'`.
   A root zone (`b === ''`) matches every pathname.
3. The longest matching base path wins. Uniqueness (§ 6.1) makes the winner unique.
4. `rest` is the pathname with `b` removed, plus the query and fragment. An empty remainder
   becomes `/`.
5. No match returns `null`.

Rule 2's segment boundary is the reason `/iamx/users` does not match the zone at `/iam`.

### 6.4 Rendering

```ts
type ZoneLinkProps = LinkProps & Omit<AnchorHTMLAttributes<HTMLAnchorElement>, 'href'> & { ref?: Ref<HTMLAnchorElement> };
function ZoneLink(props: ZoneLinkProps): ReactElement | null;
```

| Resolution | Output |
|---|---|
| `null` (no zone matches) | Nothing (`null`). |
| Same zone as `useZone().zone` | `<NextLink href={rest} {...rest props}>`. Next adds the base path back (F3). Prefetch keeps Next's default. |
| Another zone | `<a href={href} {...rest props}>`, with the href unchanged. |

- `ZoneLinkProps` is a superset of `@paigasus/ui`'s `LinkProps`, so `ZoneLink` is assignable to
  `LinkComponent`. The app injects it: `<LinkProvider link={ZoneLink}>`. Then every
  `@paigasus/ui` link becomes zone-safe too. Today every `@paigasus/ui` link is a raw
  `next/link` (F4), so a `@paigasus/ui` link to another zone would cause the AC 1 bug.
- Consequence to state in the README: under this injection, every `@paigasus/ui` href must also
  be a full path. A base-path-relative href such as `/users` matches no zone (unless a root zone
  exists) and renders nothing.
- `ZoneLink` forwards `ref` and all anchor attributes, because Radix `asChild` (the switcher
  items, § 8.3) passes a ref, `role`, `tabIndex`, event handlers and `data-*` attributes to its
  child.

### 6.5 Auth links are never `ZoneLink`

The login and logout targets are route handlers, not pages. A `next/link` to them starts an RSC
fetch, and login rejects any request whose `Sec-Fetch-Mode` is not `navigate` (F8). So:

- "Sign in" is a plain `<a href={`${basePath}/auth/login`}>`.
- "Sign out" is a native `<form method="POST" action={`${basePath}/auth/logout`}>` with a submit
  button. It is not `fetch()`, because the response is a redirect to the IdP that only a
  document navigation can follow.

`basePath` comes from `useZone()`.

---

## 7. Navigation and the three service states

### 7.1 `NavEntryState` and `navStateOf`

```ts
type NavEntryState =
  | { readonly state: 'absent' }
  | { readonly state: 'available' }
  | { readonly state: 'degraded'; readonly service: string; readonly reason: DegradedReason };

function navStateOf(serviceState: ServiceState): NavEntryState;
```

The app resolves each entry's state on the server, for example with
`discovery.getServiceState('gateway', token)`, and passes the result of `navStateOf()` as a prop.
`navStateOf` drops the descriptor (version and capability list). No descriptor data goes to the
browser.

An app that gates an entry on one capability, not on the whole service, composes
`<Capability need=… degraded={(reason) => …}>` around the entry itself. Discovery's `degraded`
render prop names this nav as its expected first user (`react.tsx`). This package does not need
to know which of the two ways the app used.

### 7.2 `PrimaryNav`

```ts
type NavEntry = {
  readonly href: string;         // full path, § 6.2
  readonly label: string;
  readonly state: NavEntryState;
  readonly requires?: RoleGrantRef; // cosmetic gate through can()
};

function PrimaryNav(props: { entries: readonly NavEntry[] }): ReactElement;
```

It renders `<nav aria-label="Primary"><ul>`. For each entry, the first rule that applies decides:

1. `resolveZone(entry.href)` is `null` (unconfigured zone) → nothing. **AC 3, first half.**
2. `state: 'absent'` → nothing.
3. `requires` is set and `can(session, requires)` is false → nothing. This is cosmetic only (F7).
4. `state: 'degraded'` → the disabled entry (§ 7.3). **AC 3, second half.**
5. `state: 'available'` → `<ZoneLink href>`, with `aria-current="page"` when the entry is active
   (§ 7.4).

Rule 1 comes first, so an entry for an unconfigured zone renders nothing even when the app
passed `degraded`. An unconfigured zone is "absent" by definition.

Rule 3 comes before rule 4, so a user without the role never sees an outage notice for a
feature that they cannot use.

`PrimaryNav` calls `useSession()` unconditionally, as the rules of hooks require. It is rendered
only inside `AppShell`, which is always under a `SessionProvider` (§ 8.1).

### 7.3 The degraded entry

```html
<span role="link" aria-disabled="true" tabindex="0" aria-describedby="{id}">
  Gateway
  <span id="{id}">gateway is not answering (timed out)</span>
</span>
```

- There is **no `href` and no `<a>`**. Nothing can navigate, before or after hydration, so there
  is nothing to block. This differs from `CapabilityDisabled`, which must disable arbitrary
  children that can contain links (F11).
- The reason text is `reasonText(service, reason)` from `@paigasus/discovery/disabled` (§ 9.2).
  It is **visible**, in a small muted style, and it is also the accessible description. AC 3 asks
  for "disabled with a reason". `CapabilityDisabled` hides its reason visually, which does not
  meet AC 3 for a sighted user.
- `tabindex="0"` keeps the entry in the tab order, so a keyboard user can reach it and hear the
  reason. `role="link"` plus `aria-disabled="true"` is the WAI-ARIA pattern for a disabled link.
  The entry has no key handler, so Enter does nothing.

### 7.4 The active entry

`usePathname()` (from `next/navigation`) gives the current pathname without the base path. An
`available` entry is active when its href resolves to the current zone and its `rest` pathname
(`p`) meets one of these conditions:

- `pathname === p`, or
- `p !== '/'` and `pathname` starts with `p + '/'`.

A cross-zone entry is never active. That `usePathname()` excludes the base path is an assumption
until the fixture measures it (§ 10.3, E7).

---

## 8. The shell components

### 8.1 `AppShell` — the authenticated shell

```ts
function AppShell(props: {
  brand: { label: string; href: string };  // href is a full path
  nav: readonly NavEntry[];
  switchers?: readonly SwitcherProps[];
  children: ReactNode;
}): ReactElement;
```

It renders:

- a skip link, "Skip to content", to `#main`;
- `<header>` (the banner landmark) with the brand as a `ZoneLink`, `PrimaryNav`, the switchers,
  and `UserMenu`;
- `<main id="main" tabindex="-1">{children}</main>`.

`AppShell` must be rendered under both `ZoneProvider` and `SessionProvider`. `UserMenu` calls
`useSession()`, so `AppShell` outside a `SessionProvider` throws (F6). That is intended: it makes
a missing provider loud.

`AppShell` takes no breadcrumbs. A root layout renders the shell once for many pages, and only a
page knows its own trail. A page renders `<Breadcrumbs>` itself (§ 8.4).

### 8.2 `PublicShell` — the shell for an unauthenticated visitor

```ts
function PublicShell(props: { brand: { label: string; href: string }; children: ReactNode }): ReactElement;
```

It renders the header with the brand and a "Sign in" link (§ 6.5), and `<main>`. It renders no
nav, no switcher and no user menu, and it **never calls `useSession()`**. So it works with no
`SessionProvider` at all. **AC 4.**

The split into two components is structural. A single shell with a `session | null` prop would
call `useSession()` conditionally, or it would need a fake anonymous `SessionView`, which F5 and
F6 do not provide for.

The app decides which shell to render: it calls `getSession()` in its server layout, then
renders `AppShell` inside `SessionProvider` when there is a session, and `PublicShell` when there
is none.

### 8.3 `Switcher`

```ts
type SwitcherItem = { readonly id: string; readonly label: string; readonly href: string };
type SwitcherProps = { readonly label: string; readonly items: readonly SwitcherItem[]; readonly currentId: string | null };
function Switcher(props: SwitcherProps): ReactElement | null;
```

- It is built on `@paigasus/ui`'s `DropdownMenu`. Radix owns focus management, arrow-key
  movement, typeahead, Escape, and focus return to the trigger. ADR-0021 decision 5 forbids a
  hand-rolled menu.
- The trigger is a button. Its accessible name is `${label}: ${current item label}`, or
  `${label}: none selected` when `currentId` is `null` or matches no item.
- Each item is `<DropdownMenuItem asChild><ZoneLink href={item.href}>`. Choosing an item is a
  navigation. The URL carries the selection (D1).
- The current item has `aria-current="true"` and a visible check mark.
- An item whose href matches no zone renders nothing (`ZoneLink` returns `null`). When no item
  remains, the whole `Switcher` renders nothing.
- One instance holds one level. An app that has orgs and teams renders two switchers.

### 8.4 `Breadcrumbs`

```ts
type Crumb = { readonly label: string; readonly href?: string };
function Breadcrumbs(props: { items: readonly Crumb[] }): ReactElement | null;
```

- `<nav aria-label="Breadcrumb"><ol>`, with a visual separator that is `aria-hidden`.
- The last item has `aria-current="page"` and no link, even when it has an `href`.
- Every other item with an `href` is a `ZoneLink`. So a breadcrumb that points into another zone
  is also a hard navigation.
- An empty list renders nothing.

### 8.5 `UserMenu`

- The trigger's accessible name is "Account menu". It shows `displayName`, or `email` when
  `displayName` is `null`, or "Account" when both are `null`.
- The menu shows `displayName` and `email` in a `DropdownMenuLabel`, then a separator, then
  "Sign out".
- "Sign out" is a `DropdownMenuItem asChild` around a `<button type="submit">`. The button is
  inside the logout `<form>` (§ 6.5), and the form is inside the menu content. Radix dispatches a
  click on Enter or Space, so the keyboard path submits the same form.

### 8.6 Styling

Components use Tailwind utility classes on `@paigasus/ui`'s design tokens. Each consuming app
needs its own `@source` line that covers `ts/packages/paigasus-app-shell/src`. This is the same
obligation that `@paigasus/ui` has (CLAUDE.md, "Tailwind v4's automatic scan root"). Without the
line, the classes disappear only in a production build. The README states the obligation, and
SMA-511 adds the line to the console.

---

## 9. Changes to neighbouring packages

### 9.1 `@paigasus/next-config`

- `BOUNDARY_SCOPES['packages/paigasus-app-shell']` becomes `'exists'`.
- The `paigasus/boundaries/app-shell` rule also bans three `server-only` entries:
  `@paigasus/discovery/server`, `@paigasus/discovery/react`, and `@paigasus/next-config/runtime`.
  Each one makes a client build fail through `server-only`. The lint rule reports the mistake
  at the import line, which is earlier and clearer than a build failure.
- `tests/boundaries.test.ts` gains DENIED rows for the three new entries (and a subpath form of
  each) and ALLOWED rows for `@paigasus/discovery/types`, `@paigasus/discovery/disabled` and
  `next/link`.
- `SOURCE_ONLY_PACKAGES` gains `@paigasus/app-shell`, `@paigasus/auth` and
  `@paigasus/discovery`. An app that renders the shell compiles this package's TypeScript source
  and the source of the two packages it imports (F13). SMA-511 needs all three.

### 9.2 `@paigasus/discovery`

- `package.json` `exports` gains `"./disabled": "./src/disabled.tsx"`. This entry is
  client-safe: `disabled.tsx` is `'use client'` and imports only React and `./types.js`.
- `tests/structure/exports.test.ts` changes from "exactly three subpaths" to exactly four, and it
  asserts that `src/disabled.tsx` does not import `server-only`.
- The README section "Consuming from `@paigasus/app-shell`" changes. The rule stays ("the app
  composes `<Capability>`"), and the section adds that `./types` and `./disabled` are the two
  client-safe entries.

No behaviour of `@paigasus/discovery` changes.

---

## 10. Testing

### 10.1 Acceptance criteria to tests

| AC | Test | Tier |
|---|---|---|
| 1. Cross-zone navigation is a plain `<a>` | `ZoneLink` output table; E2 (one document request, zero RSC requests to the other zone); E3 (negative control) | jsdom + browser |
| 2. Same-zone navigation stays soft | E1 (no document request, `window` marker survives) | browser |
| 3. Unconfigured zone → nothing; unreachable → disabled with a reason | `PrimaryNav` state table; E5 (a click on a degraded entry does not navigate) | jsdom + browser |
| 4. Unauthenticated visitor: no nav | `PublicShell` renders with no `SessionProvider`; the rendered tree has no `nav` landmark and no user menu | jsdom |
| 5. axe passes, keyboard navigation of the switcher | axe on each shell state; user-event keyboard script (§ 10.2) | jsdom |

### 10.2 jsdom tier (`test`)

The setup copies `@paigasus/ui`: `environment: 'jsdom'`, `globals: true`, a setup file with
`@testing-library/jest-dom/vitest`, and an axe helper with the same tags and options as
`@paigasus/ui`'s `expectNoAxeViolations`. `next/link` and `next/navigation` are replaced with
`vi.mock` doubles. The `next/link` double renders `<a data-next-link href>` and records its
props, so a test can tell the two element kinds apart.

- **`resolveZone` table:** exact base path, a path under it, a segment-edge near miss
  (`/iamx`), the root zone as a fallback, the longest match against the root zone, query and
  fragment kept in `rest`, an empty remainder that becomes `/`, and every rejected href form
  from § 6.2.
- **`ZoneProvider`:** an unknown current zone throws; duplicate base paths throw; `useZone()`
  outside a provider throws.
- **`ZoneLink`:** same zone → the `next/link` double with `rest`; cross zone → a plain `<a>`
  with the full href and no `data-next-link`; no match → nothing; `ref` and `aria-*` props reach
  the element.
- **`@paigasus/ui` injection:** under `<LinkProvider link={ZoneLink}>`, a `@paigasus/ui` `Link`
  to another zone renders a plain `<a>`.
- **`PrimaryNav` state table:** one row for each rule in § 7.2, the rule-order rows (unconfigured
  plus degraded → nothing; `can()` false plus degraded → nothing), `grantsAvailable: false` (the
  entry is shown, because `can()` fails open), and `aria-current` for the active entry.
- **`navStateOf`:** each `ServiceState` variant; the output has no `descriptor` or
  `capabilities` key.
- **Degraded entry:** no `<a>` and no `href` in the subtree; the reason text is in the document
  and is the entry's accessible description; the entry is focusable.
- **`PublicShell`:** renders with no `SessionProvider` and does not throw; it has no
  `navigation` landmark named "Primary" and no account menu; the "Sign in" link points to
  `${basePath}/auth/login` and is not a `next/link` double.
- **`UserMenu`:** the name fallbacks; the form has `method="post"` and the logout action, and
  the "Sign out" control is a submit button inside it.
- **`Breadcrumbs`:** the last item is `aria-current="page"` and has no link; separators are
  `aria-hidden`; an empty list renders nothing.
- **axe (AC 5):** `AppShell` closed; with the switcher open; with the user menu open; with a
  degraded entry; `PublicShell`; `Breadcrumbs`. Each test uses `document.body` as the target,
  because Radix portals menu content to the body (F15).
- **Keyboard script (AC 5):** Tab from the document start reaches the skip link, then the brand,
  then each nav entry (the degraded one included), then the switcher trigger. Enter opens the
  switcher. ArrowDown moves focus to the next item. Escape closes the menu and returns focus to
  the trigger. Enter on an item activates it (the `next/link` double records the click).

### 10.3 Browser tier (`test-e2e`)

`tests/e2e/fixture/` is a small Next app with `basePath: '/iam'`. The `test-e2e` task runs
`next build` and then Playwright. Playwright starts `next start` on a free port and stops it at
the end. The build is a production build on purpose: `next/link` prefetches differently in
development, and the AC 1 bug is a production prefetch.

The fixture renders `ZoneProvider` with the literal map
`{ iam: '/iam', gateway: '/gateway' }`, and a `SessionProvider` with a literal `SessionView`.
It does not import `@paigasus/next-config/runtime` (§ 9.1 bans it here).

Playwright answers every request to `/gateway/**` with a stub HTML document, through
`page.route`. It records every request. An **RSC request** is one with the header `rsc: 1` or a
`_rsc` query parameter.

| # | Case | Assertion |
|---|---|---|
| E1 | Same-zone click (`/iam/users`) | The URL changes; zero requests of type `document` after the first load; a `window` marker set on the first load survives. |
| E2 | Cross-zone link (`/gateway/usage`) | After load, viewport prefetch and a hover: zero RSC requests to `/gateway/*`. A **positive control on the same page** (a same-zone `ZoneLink`) must show its RSC prefetch first, so "zero" means that prefetch ran and skipped the cross-zone link. After the click: exactly one `document` request to `/gateway/usage`, and the `window` marker is gone. |
| E3 | Negative control | A fixture page with a raw `next/link` to `/gateway/raw`. The recorder must see an RSC request to `/gateway/raw`. This proves that E2's detector can fail. |
| E4 | `@paigasus/ui` `Link` under `LinkProvider link={ZoneLink}` to `/gateway/ui` | The same assertions as E2. |
| E5 | Degraded entry | A click and an Enter key press on it: the URL does not change, and no request of type `document` is sent. |
| E6 | Sign out | Open the user menu with the keyboard, then press Enter on "Sign out": exactly one `POST` request of type `document` to `/iam/auth/logout` (stubbed). |
| E7 | Measurement | `usePathname()` on `/iam/users` returns `/users`. This confirms the assumption in § 7.4. |

**Feasibility risk.** No Next app in the repo runs a browser test yet (F16). Three points are not
measured:

- whether `next build` of an app inside `tests/` resolves this package's source and the three
  workspace packages it imports (the Turbopack root, `transpilePackages`);
- the build time in CI;
- whether the fixture's `.next` output stays out of the task hash, `eslint .` and `prettier --check .`.

The plan's first task measures all three. If the build does not work, implementation stops, and
Sven decides.

### 10.4 What the tests do not prove

- **A real second zone.** E2 proves that the IAM zone sends no RSC request to `/gateway`. It does
  not run a second Next app. The failure that ADR-0017 describes (a foreign RSC payload applied
  to the IAM router) needs two apps behind one ingress. That is SMA-513's harness.
- **Colour contrast.** jsdom axe cannot compute it (F15). The fixture has no Tailwind build.
- **The app's composition.** No test proves that an app passes `navStateOf()` output and not a
  raw `ServiceState`. The type accepts only `NavEntryState`, but a structurally wider object can
  still pass. SMA-511 owns the composition.

---

## 11. Repo gate obligations

1. **Boundary rule and transpile list** (§ 9.1).
2. **Discovery export** (§ 9.2).
3. **`moon.yml`** for `paigasus-app-shell-ts`:
   - `dependsOn: ['paigasus-ui-ts', 'paigasus-auth-ts', 'paigasus-discovery-ts']`;
   - `build`, `typecheck` and `test` append `/ts/packages/paigasus-{ui,auth,discovery}/src/**/*`
     to their inputs. They append; they do not use `options.merge: replace` (the SMA-503
     defect);
   - a `test-e2e` task (`next build` of the fixture, then `playwright test`), with
     `options.cache: false`, and with the same upstream inputs plus the fixture.
   `dependsOn` schedules; only `inputs` select (CLAUDE.md). Without the inputs, a change to
   `@paigasus/ui` would not re-run this package's tests.
4. **`ci/affected-graph/run.sh`:**
   - A new pair, `app-shell->app-shell-tasks` and a second anchor on the other side of the
     `sources` glob (for example `src/zone/resolve.ts` and `src/shell/switcher.tsx`). This
     follows the SMA-509 precedent.
   - Re-baseline every strict case whose anchor lies in a source that this package's inputs now
     name: `ui->console`, `ui-components->console`, `auth->auth-tasks`,
     `discovery->discovery-tasks`, `discovery-adapters->discovery-tasks`. Each case gains
     `paigasus-app-shell-ts:build`, `:test` and `:test-e2e`.
   - `contracts->proto` probably gains `paigasus-app-shell-ts`, through the
     `dependsOn` chain discovery → proto. The plan measures it.
   - Update the comment that counts the projects that declare `test-e2e`.
   - Every expected set is **measured** with the traversal that `_assert_task_case_impl` uses. It
     is not typed by hand.
5. **`.github/workflows/ci.yml`:** the Playwright Chromium install step gains a line for this
   package. Playwright resolves from each package's own `node_modules/.bin`.
6. **Ignore files:** the fixture's `.next/` is gitignored (so the Moon hasher skips it), and it is
   ignored by ESLint and Prettier.
7. **README** with: the purpose, the href contract and its consequence for `@paigasus/ui`
   injection (§ 6.4), the auth-link rule (§ 6.5), the provider requirements (§ 8.1, § 8.2), the
   `@source` obligation (§ 8.6), and what the tests do not prove (§ 10.4).

---

## 12. Decisions rejected

| Rejected | Reason |
|---|---|
| An explicit `zone` prop with a zone-relative href | It cannot be injected into `@paigasus/ui` (`LinkProps` has no zone), so `@paigasus/ui` links stay raw `next/link`. The code would then have two href formats (D4). |
| A `zoneHref(zone, path)` helper | Not needed yet (YAGNI). |
| Memberships in `SessionView` | This changes `@paigasus/auth`'s sanitization contract and puts tenancy state in Redis. The URL can carry the selection (D1). |
| A silent `<a>` fallback for a malformed href | It hides the bug until a user clicks the link. |
| Reusing `CapabilityDisabled` for the degraded entry | Its reason is visually hidden (F11). It also exists to disable arbitrary children that can contain links. A nav entry without an `href` has nothing to disable. |
| One shell component with `session: SessionView \| null` | A conditional `useSession()` call, or a fake anonymous session (§ 8.2). |
| Breadcrumbs as an `AppShell` prop | A root layout cannot know a page's trail (§ 8.1). |
| A development-server fixture | Next prefetches differently in development. The bug is a production prefetch (§ 10.3). |
| Wiring the shell into `paigasus-console` in this PR | That is SMA-511's scope, and the console has no auth yet (D2). |

---

## 13. Challenge log

(Filled in after the adversarial challenge.)
