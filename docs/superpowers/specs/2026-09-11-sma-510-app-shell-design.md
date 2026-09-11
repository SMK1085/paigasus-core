# SMA-510 — `@paigasus/app-shell`: header, navigation and cross-zone linking

**Status:** approved design, revision 2 (after adversarial challenge)
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
- Two small changes to neighbouring packages: a plain-object zone map in
  `@paigasus/next-config` (§ 9.1), and in `@paigasus/discovery` one client-safe export plus the
  branch table of `<Capability>` moved into a shared pure function (§ 9.2).
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
- A Tailwind sentinel class for this package. It has no guard to feed until a console consumes
  the package. SMA-511 extends `ci/tailwind-source` when it adds the `@source` line.
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
| F3 | `next/link` adds the app's `basePath` to its rendered href **and to its prefetch URL**. From the `/iam` zone, a raw `<Link href="/gateway/raw">` renders and prefetches `/iam/gateway/raw`. An absolute same-origin href skips `addBasePath`. A plain `<a>` gets no base path. | Next 16.3.4 `dist/client/app-dir/link.js:375-378`, `dist/client/components/app-router-utils.js:35` |
| F4 | `@paigasus/ui` defines the navigation contract: `LinkProps = { href; className?; children? }`, `LinkProvider`, `useLinkComponent`, `Link`. **The context default is a plain `<a>`.** The console injects raw `next/link` today (`app/providers.tsx`). | `ts/packages/paigasus-ui/src/nav/link.tsx:20-32` |
| F5 | `SessionView` is `{ principalPrn, displayName, email, grants, grantsAvailable }`. It carries **no memberships** and no current org or team. `toSessionView()` drops the server-side `memberships`. `/client` exports the `SessionView` type but not `RoleGrantRef`. | `ts/packages/paigasus-auth/src/session-view.ts`, `src/client.ts:18` |
| F6 | `useSession()` throws `UseSessionOutsideProviderError` when no `SessionProvider` is above it. `SessionProvider` takes a non-null `SessionView`. | `ts/packages/paigasus-auth/src/client.ts` |
| F7 | `can(session, { scopePrn, roleKey })` is cosmetic and **fails open** when `grantsAvailable` is false. Its comment names SMA-510: failing closed "would render a console with NO navigation at all". | `ts/packages/paigasus-auth/src/client.ts:53-69` |
| F8 | Login is `GET ${basePath}/auth/login`. It rejects a request whose `Sec-Fetch-Mode` is present and is not `navigate`. Logout is `POST ${basePath}/auth/logout` with no CSRF token (`SameSite=Lax` withholds the cookie from a cross-site POST). Its response is a redirect to the IdP's end-session endpoint. Middleware redirects a visitor with no cookie to login, with `returnTo`. | `ts/packages/paigasus-auth/src/http/routes.ts:57-76, 96-121`, `src/middleware.ts:84-90` |
| F9 | Discovery's `./react` and `./server` entries are `server-only`. `<Capability>` is an async server component. `CapabilityDisabled` and `reasonText` live in `src/disabled.tsx` (`'use client'`). `./react` re-exports only `CapabilityDisabled`, not `reasonText`. `./types` is client-safe on purpose, and it holds `CapabilityKey`. | `ts/packages/paigasus-discovery/src/react.tsx:2,10`, `src/types.ts:68`, `tests/structure/exports.test.ts` |
| F10 | Discovery's README already fixes this package's contract: "App-shell exports navigation _presentation_ taking resolved state as props; the **app** composes `<Capability>` around it." `<Capability>`'s branch table is: absent → nothing; available and key listed → children; available and key not listed → nothing; degraded → disabled. | `ts/packages/paigasus-discovery/README.md:147-152`, `src/react.tsx:28-60` |
| F11 | `CapabilityDisabled` shows its reason only to assistive technology. The reason text is visually hidden. | `ts/packages/paigasus-discovery/src/disabled.tsx` |
| F12 | The boundary rule for this package already exists and is inert. It bans `@paigasus/sdk` and `@paigasus/auth/server`, value and type imports, in every file under `packages/paigasus-app-shell/`. `BOUNDARY_SCOPES` holds a "has not landed yet" placeholder for it. The `@paigasus/sdk` block uses an allowlist form with negations. | `ts/packages/paigasus-next-config/src/eslint.mjs:51-58, 99, 108-117` |
| F13 | `createNextConfig` transpiles a hand-maintained `SOURCE_ONLY_PACKAGES` list. It does not contain `@paigasus/auth` or `@paigasus/discovery`. It forces `output: 'standalone'`. | `ts/packages/paigasus-next-config/src/index.ts:12` |
| F14 | `@paigasus/ui` has no breadcrumbs, navigation-menu or tooltip component. It has `DropdownMenu*` (Radix) and `Combobox` (cmdk). | `ts/packages/paigasus-ui/src/index.ts` |
| F15 | `@paigasus/ui` runs axe through `axe-core` directly, with the helper `expectNoAxeViolations(target = document.body)`. The helper disables the `region` rule. jsdom axe cannot evaluate colour contrast. | `ts/packages/paigasus-ui/tests/axe.ts:17-21`, `README.md` "The axe limit" |
| F16 | No Next app in the repo runs a browser test today. The two `test-e2e` tasks (`auth`, `discovery`) run Playwright against a plain Node server or a Vite fixture. | `ts/packages/*/playwright.config.ts` |
| F17 | **`zones` has a null prototype.** `zoneMapFromJson` builds it with `Object.create(null)` (to keep an own `__proto__` key and to stop inherited members from reading as zones), and `getPublicConfig()` returns it unchanged. React Flight throws for a null-prototype prop ("Classes or null prototypes are not supported"). No code sends the map through Flight today. | `ts/packages/paigasus-next-config/src/runtime.ts:66-96, 253-256`; `react-server-dom-turbopack` server build |
| F18 | Radix `DropdownMenu` keeps an item registered when its `asChild` child renders `null`. Keyboard open then calls `.focus()` on `null` and throws. A mouse click closes the menu inside the click handler; with no exit animation, the content (and anything inside it) unmounts before the button's default action runs. | `@radix-ui/react-collection` `dist/index.mjs:46-63`, `@radix-ui/react-roving-focus` `:106-113, 226-230`, `@radix-ui/react-menu` `:380-388, 413-415` |
| F19 | `usePathname()` returns the pathname without the base path. Headless Chromium is not in Next's bot list, so `next/link` prefetch runs under Playwright. | Next 16.3.4 `dist/client/components/app-router.js:121`, `dist/shared/lib/router/utils/is-bot.js:36` |
| F20 | The Moon hasher does **not** skip `.next`: `.moon/workspace.yml` omits `**/.next/**` from `ignorePatterns` on purpose. A gitignore does not change that. The ESLint, Prettier and git ignores for `.next` already exist. | `.moon/workspace.yml:59-63`, `ts/eslint.config.js:14`, `ts/.prettierignore:2`, `.gitignore:16` |

F18 was read from the library source, not measured. The keyboard half is certain from the code.
The mouse half follows from HTML and React scheduling rules. § 8.5's design removes the
dependency on it, and E6 measures both paths.

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
  tsconfig.json         excludes tests/e2e/fixture
  vitest.config.ts      jsdom, like @paigasus/ui
  playwright.config.ts
  README.md
  src/
    index.ts            the one public entry
    zone/resolve.ts     resolveZone, ZoneLinkError   (no 'use client')
    zone/context.tsx    ZoneProvider, useZone        ('use client')
    zone/zone-link.tsx  ZoneLink                     ('use client')
    nav/state.ts        NavEntryState, navStateOf    (no 'use client')
    nav/primary-nav.tsx PrimaryNav                   ('use client')
    shell/app-shell.tsx AppShell                     ('use client')
    shell/public-shell.tsx PublicShell               ('use client')
    shell/switcher.tsx  Switcher                     ('use client')
    shell/user-menu.tsx UserMenu                     ('use client')
    shell/breadcrumbs.tsx Breadcrumbs                ('use client')
  tests/
    *.test.ts(x)        jsdom tier
    e2e/*.spec.ts       Playwright specs
    e2e/fixture/        a Next app with its own tsconfig.json
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
| `@paigasus/ui` | `workspace:*` | `DropdownMenu*`, `cn`, the `LinkComponent` and `LinkProps` types |
| `@paigasus/auth` | `workspace:*` | `/client` only: `useSession`, `can`, the `SessionView` type. The grant type is `SessionView['grants'][number]`, because `/client` does not export `RoleGrantRef` (F5). |
| `@paigasus/discovery` | `workspace:*` | `/types` (`ServiceState`, `DegradedReason`, `CapabilityKey`) and the new `/client` (`reasonText`, `capabilityOutcome`) |
| `next`, `react`, `react-dom` | peer, `catalog:` | `next/link`, `next/navigation` |

Test-only: `@testing-library/*`, `jsdom`, `axe-core`, `@playwright/test`, `vitest`, `typescript`,
`@types/*`, `next` and `@paigasus/next-config` (for the fixture build). All come from the
existing catalog or the workspace.

---

## 6. `ZoneProvider` and `ZoneLink`

### 6.1 `ZoneProvider`

```ts
type ZoneMap = Readonly<Record<string, string>>; // zone id -> canonical base path

function ZoneProvider(props: { zone: string; zones: ZoneMap; children: ReactNode }): ReactElement;
function useZone(): { zone: string; basePath: string; zones: ZoneMap };
```

- The app's server layout calls `getPublicConfig()` (F1) and passes `zone` and `zones` as props.
  No other value crosses to the browser. Internal service URLs never do.
- **`getPublicConfig()` must return a plain object** for this to work. Today it returns a
  null-prototype map, and React Flight throws for it (F17). § 9.1 fixes this in
  `@paigasus/next-config`.
- The map that reaches the client has `Object.prototype`, so `zones['constructor']` is defined
  there. `ZoneProvider` and `resolveZone` therefore read the map only with `Object.hasOwn` and
  `Object.entries`, never with `zones[id] === undefined`.
- `ZoneMap` is a structural type that this package defines. The package does not import
  `PublicConfig`, because that type lives in a `server-only` module, and § 9.1 bans that module
  from this package (value and type imports).
- `ZoneProvider` throws `ZoneConfigError` in two cases: `zone` is not an own key of `zones`, or
  two zones have the same base path. `@paigasus/next-config` also rejects duplicate base paths at
  startup (§ 9.1), where the operator sees a configuration error. The check here stays, because
  a literal map (a test, the fixture) does not pass through `zoneMapFromJson`.
- `useZone()` outside a provider throws `ZoneProviderMissingError`. There is no default. A
  default zone map would make every cross-zone link look same-zone, which is the AC 1 bug.

### 6.2 The href contract

The caller writes the URL as the ingress sees it: `<ZoneLink href="/gateway/usage">`,
`<ZoneLink href="/iam/users?page=2#top">`.

The href must be a path that starts with exactly one `/`. `resolveZone` throws `ZoneLinkError`
for every other form. The rules copy those of `@paigasus/auth`'s `return-to.ts:19-28`, plus a
dot-segment rule:

- an absolute URL (`https://…`, `mailto:…`, any scheme);
- a protocol-relative URL (`//host/…`);
- a backslash anywhere in the path (`/\evil.com`), because browsers read `\` as `/`;
- an ASCII control character or whitespace anywhere (`/<TAB>/evil.com`);
- a relative path (`users`, `./users`) or an empty string;
- a dot segment (`/iam/../gateway/x`). The test: the pathname after WHATWG URL normalisation
  must equal the raw pathname. Otherwise the browser would go to a different zone from the one
  the match chose.

The error does not fall back to a plain `<a>`. A silent fallback would hide a wrong href until a
user clicks it. The error message does not echo the href, because an href can carry user data.

**Failure scope.** A `ZoneLinkError` goes to the nearest React error boundary, which can be the
whole shell. That is intended: hrefs are built in code, and the jsdom tier renders every shell
href. An app that builds hrefs from data (for example switcher items) builds them from IAM ids,
which are UUIDv7 and cannot form a dot segment.

### 6.3 Resolution

`resolveZone(href, zones)` returns `{ zone, basePath, rest } | null`:

1. Validate the href (§ 6.2). Split it into pathname, and query plus fragment. Only the pathname
   takes part in the match.
2. A zone with base path `b` matches when `pathname === b` or `pathname` starts with `b + '/'`.
   A root zone (`b === ''`) matches every pathname.
3. The longest matching base path wins. Uniqueness (§ 6.1) makes the winner unique.
4. `rest` is the pathname with `b` removed, plus the query and fragment. An empty remainder
   becomes `/`.
5. No match returns `null`.

Rule 2's segment boundary is the reason `/iamx/users` does not match the zone at `/iam`.

**The root-zone limit.** When a root zone exists, rule 2 makes `resolveZone` return a zone for
every href, so a bare `ZoneLink` can never detect an unconfigured zone: `/billing/x` resolves
to the root zone. `PrimaryNav` closes this for nav entries with an explicit `zone` (§ 7.2). A
bare `ZoneLink` in a deployment with a root zone cannot. The README states the limit.

### 6.4 Rendering

```ts
type ZoneLinkProps = LinkProps & Omit<AnchorHTMLAttributes<HTMLAnchorElement>, 'href'> & { ref?: Ref<HTMLAnchorElement> };
function ZoneLink(props: ZoneLinkProps): ReactElement | null;
```

| Resolution | Output |
|---|---|
| `null` (no zone matches) | Nothing (`null`). In development only, `console.warn` once, naming the first path segment and not the full href. |
| Same zone, and `rest` is under `/auth/` | A plain `<a href={href}>`. The auth routes are route handlers (§ 6.5). |
| Same zone, any other `rest` | `<NextLink href={rest} {...other props}>`. Next adds the base path back (F3). Prefetch keeps Next's default. |
| Another zone | `<a href={href} {...other props}>`, with the href unchanged. |

- **"No match renders nothing" stays**, as the issue scope states. It is a normal runtime case:
  an IAM-only deployment has no gateway zone, and its gateway links must disappear. The
  development warning makes the other cause visible: a zone-relative href such as `/users`,
  which a Next developer writes by habit and which matches no zone.
- The list components (`PrimaryNav`, `Switcher`, `Breadcrumbs`) do not rely on this `null`. They
  call `resolveZone` first and drop an unmatched item before they render any wrapper element
  (F18, § 8.3).
- `ZoneLinkProps` is a superset of `@paigasus/ui`'s `LinkProps`, so `ZoneLink` is assignable to
  `LinkComponent`. The app injects it: `<LinkProvider link={ZoneLink}>`. Then every
  `@paigasus/ui` link becomes zone-safe too. Today every `@paigasus/ui` link is a raw
  `next/link` (F4), so a `@paigasus/ui` link to another zone would cause the AC 1 bug.
- Consequence to state in the README: under this injection, every `@paigasus/ui` href must also
  be a full path.
- `ZoneLink` forwards `ref` and all anchor attributes. Next 16's `Link` takes `ref` as a prop and
  spreads the other props onto its `<a>`, so the `ref`, `role`, `tabIndex`, handlers and `data-*`
  attributes that Radix `asChild` passes reach the DOM.
- `ZoneLinkProps` has no `prefetch`, `replace` or `scroll`. Next's defaults apply. There is no
  consumer for them yet.

### 6.5 Auth links are never soft

The login and logout targets are route handlers, not pages. A `next/link` to them starts an RSC
fetch, and login rejects any request whose `Sec-Fetch-Mode` is not `navigate` (F8). So:

- "Sign in" is a plain `<a href={`${basePath}/auth/login`}>`. It carries no `returnTo`. For a
  protected page, the middleware redirect already adds one (F8). A visitor who clicks "Sign in"
  on a public page lands on the login default, `${basePath}/`.
- "Sign out" submits a native `<form method="POST" action={`${basePath}/auth/logout`}>` (§ 8.5).
  It is not `fetch()`, because the response is a redirect to the IdP that only a document
  navigation can follow.
- `ZoneLink` renders a plain `<a>` for any same-zone href under `/auth/` (§ 6.4), so an auth
  link written through `ZoneLink` is also safe.

`basePath` comes from `useZone()`.

---

## 7. Navigation and the three service states

### 7.1 `NavEntryState` and `navStateOf`

```ts
type NavEntryState =
  | { readonly state: 'absent' }
  | { readonly state: 'available' }
  | { readonly state: 'degraded'; readonly service: string; readonly reason: DegradedReason };

function navStateOf(serviceState: ServiceState, need?: CapabilityKey): NavEntryState;
```

The app resolves each entry's state on the server, for example with
`discovery.getServiceState('iam', token)`, and passes `navStateOf(…)` as a prop.

`navStateOf` applies `<Capability>`'s branch table (F10), so an app does not copy it by hand:

| `serviceState` | `need` | Result |
|---|---|---|
| `absent` | any | `absent` |
| `available` | not given | `available` |
| `available` | listed in `capabilities` | `available` |
| `available` | not listed | `absent` (the build does not provide the feature; this is not an outage) |
| `degraded` | any | `degraded` with `service` and `reason` |

**There is one copy of this table, and it lives in `@paigasus/discovery`.** § 9.2 moves the
branch into a pure function, `capabilityOutcome(state, need?)`, which returns
`'hidden' | 'shown' | 'degraded'`. `<Capability>` calls it, and `navStateOf` calls it. A
parity test in this package cannot import `<Capability>`, because the boundary rule bans
`@paigasus/discovery/react` in every file here, tests included. One shared function makes a
parity test unnecessary.

When `need` is given and `serviceOf(need)` is not `state.service`, `capabilityOutcome` throws. An
`iam.*` key checked against the gateway's state is a programming error.

`navStateOf` drops the descriptor (version and capability list). No descriptor data goes to the
browser.

`PrimaryNav` renders its entries from data, so an app cannot wrap one entry in `<Capability>`.
`navStateOf(state, need)` is the capability-level path for nav entries. `<Capability>` stays the
right tool for feature UI inside a page.

### 7.2 `PrimaryNav`

```ts
type NavEntry = {
  readonly zone: string;         // the zone the entry belongs to
  readonly href: string;         // full path, § 6.2
  readonly label: string;
  readonly state: NavEntryState;
  readonly requires?: SessionView['grants'][number]; // cosmetic gate through can()
};

function PrimaryNav(props: { entries: readonly NavEntry[] }): ReactElement | null;
```

It renders `<nav aria-label="Primary"><ul>`. For each entry, the first rule that applies decides:

1. `entry.zone` is not an own key of `zones` (an unconfigured zone) → nothing. **AC 3, first
   half.** This rule uses the declared zone, not the href, so it also holds when a root zone
   exists (§ 6.3).
2. `resolveZone(entry.href)` does not return `entry.zone` → `ZoneLinkError`. The entry's href
   and its declared zone disagree, which is a programming error.
3. `state: 'absent'` → nothing.
4. `requires` is set and `can(session, requires)` is false → nothing. This is cosmetic only (F7).
5. `state: 'degraded'` → the disabled entry (§ 7.3). **AC 3, second half.**
6. `state: 'available'` → `<ZoneLink href>`, with `aria-current="page"` when the entry is the
   active one (§ 7.4).

Rule 1 comes before rule 5, so an entry for an unconfigured zone renders nothing even when the
app passed `degraded`. An unconfigured zone is "absent" by definition.

Rule 4 comes before rule 5, so a user without the role never sees an outage notice for a
feature that they cannot use.

When no entry remains, `PrimaryNav` returns `null`. It does not render an empty `nav` landmark.

`PrimaryNav` calls `useSession()` unconditionally, as the rules of hooks require. It is rendered
only inside `AppShell`, which is always under a `SessionProvider` (§ 8.1).

### 7.3 The degraded entry

```html
<li>
  <span role="link" aria-disabled="true" tabindex="0" aria-describedby="{id}">Gateway</span>
  <span id="{id}">gateway is not answering (timed out)</span>
</li>
```

- There is **no `href` and no `<a>`**. Nothing can navigate, before or after hydration, so there
  is nothing to block. This differs from `CapabilityDisabled`, which must disable arbitrary
  children that can contain links (F11).
- The reason span is a **sibling** of the `role="link"` element, not a child. As a child, it
  would be part of the accessible name and also the description, so a screen reader would read
  it twice. The accessible name is only the label ("Gateway").
- The reason text is `reasonText(service, reason)` from `@paigasus/discovery/client` (§ 9.2).
  It is **visible**, in a small muted style, and it is also the accessible description. AC 3
  asks for "disabled with a reason". `CapabilityDisabled` hides its reason visually, which does
  not meet AC 3 for a sighted user.
- `tabindex="0"` keeps the entry in the tab order, so a keyboard user can reach it and hear the
  reason. `role="link"` plus `aria-disabled="true"` is the WAI-ARIA pattern for a disabled link.
  The entry has no key handler, so Enter does nothing.

### 7.4 The active entry

`usePathname()` gives the current pathname without the base path (F19). An `available` entry is
a candidate when its href resolves to the current zone and its `rest` pathname (`p`) meets one of
these conditions:

- `pathname === p`, or
- `p !== '/'` and `pathname` starts with `p + '/'`.

Of the candidates, **only the one with the longest `p`** gets `aria-current="page"`. So
`/iam/orgs` and `/iam/orgs/teams` are never both current. A cross-zone entry is never a
candidate.

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

It renders the skip link, the header with the brand and a "Sign in" link (§ 6.5), and `<main>`.
It renders no nav, no switcher and no user menu, and it **never calls `useSession()`**. So it
works with no `SessionProvider` at all. **AC 4.**

Its pages are the public ones: a signed-out landing page, the page after logout, and error
pages. The middleware sends a visitor with no cookie away from protected pages before any shell
renders (F8). The app decides which shell to render: it calls `getSession()` in its server
layout, then renders `AppShell` inside `SessionProvider` when there is a session, and
`PublicShell` when there is none (for example a visitor whose cookie is stale).

The split into two components is structural. A single shell with a `session | null` prop would
call `useSession()` conditionally, or it would need a fake anonymous `SessionView`, which F5 and
F6 do not provide for.

### 8.3 `Switcher`

```ts
type SwitcherItem = { readonly id: string; readonly label: string; readonly href: string };
type SwitcherProps = { readonly label: string; readonly items: readonly SwitcherItem[]; readonly currentId: string | null };
function Switcher(props: SwitcherProps): ReactElement | null;
```

- It is built on `@paigasus/ui`'s `DropdownMenu`. Radix owns focus management, arrow-key
  movement, typeahead, Escape, and focus return to the trigger. ADR-0021 decision 5 forbids a
  hand-rolled menu.
- **It filters first.** It calls `resolveZone` on each item and drops the unmatched ones
  **before** it renders any `DropdownMenuItem`. A `DropdownMenuItem` whose child renders `null`
  stays registered with Radix, and opening the menu with the keyboard then throws (F18).
- When no item remains after the filter, the whole `Switcher` renders nothing.
- The trigger is a button. Its visible text is `${label}: ${current item label}`, or
  `${label}: none selected` when `currentId` is `null` or matches no remaining item. The visible
  text is also the accessible name, so the name contains the visible label (WCAG 2.5.3).
- Each remaining item is `<DropdownMenuItem asChild><ZoneLink href={item.href}>`. Choosing an
  item is a navigation. The URL carries the selection (D1).
- The current item has `aria-current="true"` and a visible check mark.
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
- An item whose `href` matches no zone is shown as plain text, without a link. The trail keeps
  its length, so the user still sees where they are.
- An empty list renders nothing.

### 8.5 `UserMenu`

- The trigger's visible text is `displayName`, or `email` when `displayName` is `null`, or
  "Account" when both are `null`. The visible text is also the accessible name (WCAG 2.5.3).
  Radix adds `aria-haspopup` and `aria-expanded`.
- The menu shows `displayName` and `email` in a `DropdownMenuLabel`, then a separator, then
  "Sign out".
- **The logout `<form>` is rendered outside the menu portal**, as a sibling of the
  `DropdownMenu` root, with a ref. "Sign out" is a `DropdownMenuItem` with
  `onSelect={() => formRef.current?.requestSubmit()}`.
- The reason: a mouse click closes the menu inside the click handler, and without an exit
  animation the portaled content unmounts before a submit button's default action runs (F18).
  Production has an exit animation today (`@paigasus/ui` `dropdown-menu.tsx:22`), but a class
  change must not silently break sign-out for mouse users. A form outside the portal stays
  mounted. `requestSubmit()` runs the same native submission as a submit button.
- E6 (§ 10.3) measures both the mouse and the keyboard path in the unstyled fixture, which has no
  exit animation.

### 8.6 Styling

Components use Tailwind utility classes on `@paigasus/ui`'s design tokens. Each consuming app
needs its own `@source` line that covers `ts/packages/paigasus-app-shell/src`. This is the same
obligation that `@paigasus/ui` has (CLAUDE.md, "Tailwind v4's automatic scan root"). Without the
line, the classes disappear only in a production build. The README states the obligation, and
SMA-511 adds the line to the console.

---

## 9. Changes to neighbouring packages

### 9.1 `@paigasus/next-config`

- **`getPublicConfig()` returns `zones: { ...full.PAIGASUS_ZONES }`**, a plain object (F17).
  Object spread defines properties and does not call the `__proto__` setter, so an own
  `__proto__` zone id survives. `runtime.test.ts:199-209` stays green, and a new assertion checks
  `Object.getPrototypeOf(zones) === Object.prototype`. The internal map that
  `assertCompiledAgreement` reads keeps its null prototype.
- **`zoneMapFromJson` rejects two zones with the same base path**, with a configuration error
  that names the two zone ids and not the value (the existing rule: operator input is withheld).
  A new test row covers it.
- `BOUNDARY_SCOPES['packages/paigasus-app-shell']` becomes `'exists'`.
- `paigasus/boundaries/app-shell` changes to the **allowlist form** that the `@paigasus/sdk`
  block uses (F12). Within the `@paigasus/*` scope, this package may import only
  `@paigasus/ui`, `@paigasus/auth/client`, `@paigasus/discovery/types` and
  `@paigasus/discovery/client`. The one exception is `@paigasus/next-config` (root only, not
  `/runtime`), and only under `tests/e2e/fixture/` (the fixture's `next.config.ts`). Everything
  else in the scope is banned, which includes `@paigasus/sdk`, `@paigasus/auth/server`,
  `@paigasus/auth/middleware`, `@paigasus/discovery/{server,react}`,
  `@paigasus/next-config/runtime`, `@paigasus/proto` and `@paigasus/kernel`.
- `tests/boundaries.test.ts` gains DENIED rows (each banned entry, and a subpath form where one
  exists) and ALLOWED rows (each allowed entry, `next/link`, and the fixture exception).
- The liveness test in `tests/boundaries.test.ts:158-179` gains one rule: a scope whose
  directory exists must say `'exists'`. Today a stale "has not landed yet" note passes once the
  directory exists.
- `SOURCE_ONLY_PACKAGES` gains `@paigasus/app-shell`, `@paigasus/auth` and
  `@paigasus/discovery`. An app that renders the shell compiles this package's TypeScript source
  and the source of the two packages it imports (F13). The fixture builds through
  `createNextConfig`, so the fixture build proves that the list is sufficient (§ 10.3).

### 9.2 `@paigasus/discovery`

- **A new pure function, `capabilityOutcome(state: ServiceState, need?: CapabilityKey): 'hidden' | 'shown' | 'degraded'`**,
  in `src/core/outcome.ts` (no directive, no `server-only`). It holds the branch table that
  `<Capability>` has inline today (`react.tsx:43-60`), including the `serviceOf(need)` check
  from § 7.1. `<Capability>` calls it. The existing `tests/capability.test.tsx` rows prove that
  `<Capability>`'s behaviour does not change, and a new table test covers the function.
- **A new client-safe entry, `"./client": "./src/client.ts"`**, and `_comment_exports` explains
  it. `src/client.ts` carries no directive. It re-exports `reasonText` and `CapabilityDisabled`
  from `./disabled.js` (which stays `'use client'`) and `capabilityOutcome` from
  `./core/outcome.js`. A server module can call `capabilityOutcome` through it (`navStateOf` runs
  in a server layout), and a client module can call `reasonText`.
- `tests/structure/exports.test.ts` changes from "exactly three subpaths" to exactly four. It
  asserts that `src/client.ts`, `src/disabled.tsx` and `src/core/outcome.ts` do not import
  `server-only`, and that their import sets are exactly `{./disabled.js, ./core/outcome.js}`,
  `{react, ./types.js}` and `{./types.js, ./state.js}` (or what the file actually needs; the plan
  fixes the sets). A later import that pulls a server module into the entry then fails the test.
- The README section "Consuming from `@paigasus/app-shell`" changes. The rule stays ("the app
  composes `<Capability>`" for feature UI), and the section adds that `./types` and `./client`
  are the two client-safe entries and that `navStateOf` covers nav entries.

No behaviour of `@paigasus/discovery` changes.

---

## 10. Testing

### 10.1 Acceptance criteria to tests

| AC | Test | Tier |
|---|---|---|
| 1. Cross-zone navigation is a plain `<a>` | `ZoneLink` output table; E2 (no RSC request outside the allowlist, one document request on click); E3 (negative control on the same page) | jsdom + browser |
| 2. Same-zone navigation stays soft | E1 (no document request, `window` marker survives) | browser |
| 3. Unconfigured zone → nothing; unreachable → disabled with a reason | `PrimaryNav` state table, including a root zone; E5 (a degraded entry does not navigate) | jsdom + browser |
| 4. Unauthenticated visitor: no nav | `PublicShell` renders with no `SessionProvider`; the rendered tree has no nav landmark and no user menu | jsdom |
| 5. axe passes, keyboard navigation of the switcher | axe on each shell state; user-event keyboard script (§ 10.2) | jsdom |

### 10.2 jsdom tier (`test`)

The setup copies `@paigasus/ui`: `environment: 'jsdom'`, `globals: true`, a setup file with
`@testing-library/jest-dom/vitest`, and an axe helper with the same tags as `@paigasus/ui`'s
`expectNoAxeViolations`. The helper takes a `region` option: the `region` rule is **enabled**
for the closed `AppShell` and for `PublicShell`, because the shell is the landmark structure.
It stays disabled only for tests that render a component outside a shell.

`next/link` and `next/navigation` are replaced with `vi.mock` doubles. The `next/link` double
renders `<a data-next-link href>` and records its props, so a test can tell the two element
kinds apart.

- **`resolveZone` table:** exact base path, a path under it, a segment-edge near miss
  (`/iamx`), the root zone as a fallback, the longest match against the root zone, query and
  fragment kept in `rest`, an empty remainder that becomes `/`, a map whose keys include
  `constructor` and `__proto__`, and every rejected href form from § 6.2 (including `/\evil.com`,
  a tab, and `/iam/../gateway/x`).
- **`ZoneProvider`:** an unknown current zone throws; `zone: 'constructor'` with a map that lacks
  that own key throws; duplicate base paths throw; `useZone()` outside a provider throws.
- **`ZoneLink`:** same zone → the `next/link` double with `rest`; same zone under `/auth/` → a
  plain `<a>`; cross zone → a plain `<a>` with the full href and no `data-next-link`; no match →
  nothing, plus one development warning that does not contain the full href; `ref` and `aria-*`
  props reach the element.
- **`@paigasus/ui` injection (both directions):** under `<LinkProvider link={ZoneLink}>`, a
  `@paigasus/ui` `Link` to another zone renders a plain `<a>`, **and a `@paigasus/ui` `Link` to
  the same zone renders the `next/link` double**. The second row is the one that can fail: the
  `@paigasus/ui` context default is already a plain `<a>` (F4), so the first row passes with no
  injection at all.
- **`navStateOf` table:** each row of § 7.1; the output has no `descriptor` or `capabilities`
  key; a key from another service throws. The branch itself is `capabilityOutcome`, which
  `@paigasus/discovery` tests (§ 9.2). There is one copy, so no parity test is needed.
- **`PrimaryNav` state table:** one row for each rule in § 7.2; the rule-order rows (unconfigured
  plus degraded → nothing; `can()` false plus degraded → nothing); an unconfigured zone with a
  root zone present → nothing; href and declared zone disagree → `ZoneLinkError`;
  `grantsAvailable: false` → the entry is shown, because `can()` fails open; every entry
  filtered → no `nav` element at all; `aria-current` on the longest match only.
- **Degraded entry:** no `<a>` and no `href` in the subtree; the reason text is visible; the
  entry's accessible name is exactly the label (`toHaveAccessibleName('Gateway')`); its
  accessible description is the reason; it is focusable.
- **`Switcher`:** an unmatched item is not rendered; with one unmatched item **first** in the
  list, opening the menu with the keyboard puts focus on the first remaining item and does not
  throw; all items unmatched → nothing; the trigger's accessible name contains its visible text.
- **`PublicShell`:** renders with no `SessionProvider` and does not throw; it has no `navigation`
  landmark named "Primary" and no account menu; the "Sign in" link points to
  `${basePath}/auth/login` and is not a `next/link` double.
- **`UserMenu`:** the name fallbacks; the accessible name equals the visible text; the form has
  `method="post"` and the logout action, and it is **not** inside the menu content; selecting
  "Sign out" calls `requestSubmit` on it (spied, because jsdom does not submit).
- **`Breadcrumbs`:** the last item is `aria-current="page"` and has no link; separators are
  `aria-hidden`; an unmatched `href` renders as text; an empty list renders nothing.
- **axe (AC 5):** `AppShell` closed; with the switcher open; with the user menu open; with a
  degraded entry; `PublicShell`; `Breadcrumbs`. The target is `document.body`, because Radix
  portals menu content to the body (F15).
- **Keyboard script (AC 5):** Tab from the document start reaches the skip link, then the brand,
  then each nav entry (the degraded one included), then the switcher trigger. Enter opens the
  switcher. ArrowDown moves focus to the next item. Escape closes the menu and returns focus to
  the trigger. Enter on an item activates it (the `next/link` double records the click).

### 10.3 Browser tier (`test-e2e`)

**The fixture.** `tests/e2e/fixture/` is a small Next app with its own `tsconfig.json`. Its
`next.config.ts` calls `createNextConfig({ zone: 'iam', basePath: '/iam', … })` from
`@paigasus/next-config`, so the build also proves that `SOURCE_ONLY_PACKAGES` (§ 9.1) is
sufficient. `createNextConfig` forces `output: 'standalone'`, so Playwright starts the fixture
with the standalone `server.js` on a free port, not with `next start`, and stops it at the end.

The build is a production build on purpose: `next/link` prefetches differently in development,
and the AC 1 bug is a production prefetch.

The fixture renders `ZoneProvider` with the literal map `{ iam: '/iam', gateway: '/gateway' }`,
and a `SessionProvider` with a literal `SessionView`. It does not import
`@paigasus/next-config/runtime` (§ 9.1 bans it here).

**Module identity.** The fixture imports `@paigasus/app-shell` and `@paigasus/ui` so that each
resolves to exactly one module instance. Two copies of `@paigasus/ui` would give two
`LinkContext` objects, and the injection would silently do nothing. How the fixture reaches the
package (package name, a path alias, or a relative import) is a measurement in the plan's first
task. E4's same-zone row is the test that fails if there are two copies.

**The recorder.** Playwright answers every request under `/gateway/**` and `/iam/gateway/**` with
a stub HTML document, and `POST /iam/auth/logout` with a stub, through `page.route`. It records
every request. An **RSC request** is one with the header `rsc: 1` or a `_rsc` query parameter.

**The allowlist rule.** A page declares the set of pathnames that it may send an RSC request
to: its own same-zone targets. **Any RSC request whose pathname is not in the page's allowlist
fails the test.** This rule does not depend on where a wrong link sends its prefetch. A
`ZoneLink` that wrongly uses `next/link` for `/gateway/usage` prefetches `/iam/gateway/usage`
(F3). A filter on `/gateway/*` would not see that request. The allowlist rule does.

**The main page** carries, in this DOM order:

1. the links under test: the cross-zone `ZoneLink` to `/gateway/usage` and the `@paigasus/ui`
   `Link` to `/gateway/ui` under `<LinkProvider link={ZoneLink}>`;
2. the positive controls: a same-zone `ZoneLink` to `/iam/users` and a same-zone `@paigasus/ui`
   `Link` to `/iam/settings` (under the same provider);
3. the negative control: a raw `next/link` to `/gateway/raw`.

Its allowlist is `{ /iam/users, /iam/settings, /iam/gateway/raw }`. `/iam/gateway/raw` is on the
list only so that the negative control's request does not fail the run. E3 then asserts that the
request exists.

| # | Case | Assertion |
|---|---|---|
| E1 | Same-zone click (`/iam/users`) | The URL changes; zero requests of type `document` after the first load; a `window` marker set on the first load survives. |
| E2 | Cross-zone `ZoneLink` (`/gateway/usage`) | Load the main page, scroll, and hover each link under test. **Wait until the RSC requests for all three controls have arrived** (`/iam/users`, `/iam/settings`, `/iam/gateway/raw`). The controls come after the links under test in DOM order, so their requests show that Next processed the prefetch queue past the links under test. Then assert the allowlist rule. Click: exactly one request of type `document`, to `/gateway/usage`, and the `window` marker is gone. |
| E3 | Negative control | On the same page load as E2, an RSC request to `/iam/gateway/raw` exists. This proves that the recorder sees the exact failure that a raw `next/link` to another zone causes. |
| E4 | `@paigasus/ui` `Link` under `LinkProvider link={ZoneLink}` | Cross zone (`/gateway/ui`): the E2 assertions. Same zone (`/iam/settings`): its RSC prefetch exists, and a click sends no `document` request. The same-zone half fails if the injection did not take effect or if the fixture bundles two copies of `@paigasus/ui`. |
| E5 | Degraded entry | A click and an Enter key press on it: the URL does not change, and no request of type `document` is sent. |
| E6 | Sign out, both paths | (a) Mouse: open the user menu, click "Sign out". (b) Keyboard: focus the trigger, Enter, move to "Sign out", Enter. Each path: exactly one `POST` request of type `document` to `/iam/auth/logout`. The fixture has no exit animation, so path (a) fails if the form unmounts with the menu (F18). |
| E7 | Measurement | `usePathname()` on `/iam/users` returns `/users` (F19). |

**Feasibility risk.** No Next app in the repo runs a browser test yet (F16). These points are not
measured:

- whether `next build` of an app inside `tests/` resolves this package's source and the
  workspace packages it imports (the Turbopack root, `transpilePackages`, module identity);
- whether the standalone `server.js` path is stable for a fixture under a package;
- the build time in CI;
- whether the fixture's `.next` stays out of other tasks (§ 11.6).

The plan's first task measures all four. If the build does not work, implementation stops, and
Sven decides.

### 10.4 What the tests do not prove

- **A real second zone.** E2 proves that the IAM zone sends no RSC request outside its own
  targets. It does not run a second Next app. The failure that ADR-0017 describes (a foreign RSC
  payload applied to the IAM router) needs two apps behind one ingress. That is SMA-513's
  harness.
- **Colour contrast.** jsdom axe cannot compute it (F15). The fixture has no Tailwind build.
- **The Flight handoff in an app.** § 9.1's test proves that `getPublicConfig()` returns a
  plain object. The fixture uses a literal map, so no test here sends the real map through
  Flight. SMA-511's app does.
- **The app's composition.** No test proves that an app passes `navStateOf()` output and not a
  raw `ServiceState`. The type accepts only `NavEntryState`, but a structurally wider object can
  still pass. SMA-511 owns the composition.

---

## 11. Repo gate obligations

1. **`@paigasus/next-config`** changes (§ 9.1).
2. **`@paigasus/discovery`** export (§ 9.2).
3. **`moon.yml`** for `paigasus-app-shell-ts`. Every list below is the full list. `build`,
   `typecheck` and `test` inherit their base inputs from `.moon/tasks/typescript-project.yml` and
   **append**; they never use `options.merge: replace` (the SMA-503 defect). `test-e2e` is a new
   task name, so it inherits nothing, and its list is written out in full.
   - `dependsOn: ['paigasus-ui-ts', 'paigasus-auth-ts', 'paigasus-discovery-ts', 'paigasus-next-config-ts']`.
   - Upstream inputs, called **U** below: `/ts/packages/paigasus-{ui,auth,discovery}/src/**/*` and
     `/ts/packages/paigasus-{ui,auth,discovery}/package.json`. The manifests are inputs because
     § 9.2 changes discovery's `exports`, and the console lists the ui manifest for the same
     reason.
   - `build` and `typecheck`: inherited inputs + U.
   - `test`: inherited inputs + U + `vitest.config.ts`.
   - `test-e2e`: `@group(sources)`, `@group(tests)`, `package.json`, `tsconfig.json`,
     `playwright.config.ts`, `tests/e2e/fixture/**/*`, `!tests/e2e/fixture/.next/**`,
     `/ts/pnpm-lock.yaml`, `/ts/tsconfig.base.json`, U, and
     `/ts/packages/paigasus-next-config/src/**/*` plus its `package.json` (the fixture builds
     through `createNextConfig`). `options.cache: false`, as in the two existing `test-e2e`
     tasks.
   - `dependsOn` schedules; only `inputs` select (CLAUDE.md). Without U, a change to
     `@paigasus/ui` would not re-run this package's tests.
4. **`ci/affected-graph/run.sh`:**
   - A new pair, `app-shell->app-shell-tasks` and a second anchor on the other side of the
     `sources` glob (for example `src/zone/resolve.ts` and `src/shell/switcher.tsx`). This
     follows the SMA-509 precedent.
   - Re-baseline every strict case whose anchor lies in a source that this package's inputs now
     name: `ui->console`, `ui-components->console`, `auth->auth-tasks`,
     `discovery->discovery-tasks`, `discovery-adapters->discovery-tasks`. Each case gains the
     `paigasus-app-shell-ts` tasks that the measurement shows.
   - `contracts->proto` gains `paigasus-app-shell-ts`: `--downstream deep` follows `dependsOn`,
     and the chain is app-shell → discovery → proto.
   - A `next-config` source anchor now selects `paigasus-app-shell-ts:test-e2e`. If an existing
     strict case anchors there, it is re-baselined too.
   - Update the comment that counts the projects that declare `test-e2e` (three after this
     change).
   - Every expected set is **measured** with the traversal that `_assert_task_case_impl` uses. It
     is not typed by hand.
5. **`.github/workflows/ci.yml`:** the Playwright Chromium install step gains a line for this
   package. Playwright resolves from each package's own `node_modules/.bin`.
6. **The fixture's `.next` and generated files** (F20):
   - The package `tsconfig.json` excludes `tests/e2e/fixture/**`. The fixture has its own
     `tsconfig.json`, which `next build` type-checks. So `:typecheck` never reads
     `.next/types/**` while `:test-e2e` writes it, and the two can run at the same time.
   - The inherited `test` input `@group(tests)` covers `tests/**/*`, and the Moon hasher does not
     skip `.next` (F20). The `test` and `test-e2e` tasks therefore add
     `!tests/e2e/fixture/.next/**`. `ts:lint` and `ts:fmt` use `packages/*/tests/**/*` in
     `ts/moon.yml`; the plan measures whether they need the same negation there, and whether
     `repo:next-public-free`'s negation (`moon.yml:921-926, 939`) must be extended to this path.
   - The fixture's `next-env.d.ts` is gitignored, because `next build` writes it. It is not
     tracked, so `repo:next-env-drift` (which checks only the console) needs no change.
7. **README** with: the purpose, the href contract and its consequence for `@paigasus/ui`
   injection (§ 6.4), the root-zone limit (§ 6.3), the auth-link rule (§ 6.5), the provider
   requirements (§ 8.1, § 8.2), the `@source` obligation (§ 8.6), and what the tests do not prove
   (§ 10.4).

---

## 12. Decisions rejected

| Rejected | Reason |
|---|---|
| An explicit `zone` prop on `ZoneLink` with a zone-relative href | It cannot be injected into `@paigasus/ui` (`LinkProps` has no zone), so `@paigasus/ui` links stay raw `next/link`. The code would then have two href formats (D4). `NavEntry` carries a `zone` for a different reason (§ 7.2): it makes AC 3 hold with a root zone, and the href stays a full path. |
| A `zoneHref(zone, path)` helper | Not needed yet (YAGNI). |
| Memberships in `SessionView` | This changes `@paigasus/auth`'s sanitization contract and puts tenancy state in Redis. The URL can carry the selection (D1). |
| A silent `<a>` fallback for a malformed href | It hides the bug until a user clicks the link. |
| `ZoneLink` throws when no zone matches (challenge M5) | The issue scope states "unconfigured zone → renders nothing", and an IAM-only deployment needs exactly that. A development warning shows the habitual zone-relative href instead (§ 6.4). |
| Reusing `CapabilityDisabled` for the degraded entry | Its reason is visually hidden (F11). It also exists to disable arbitrary children that can contain links. A nav entry without an `href` has nothing to disable. |
| One shell component with `session: SessionView \| null` | A conditional `useSession()` call, or a fake anonymous session (§ 8.2). |
| Breadcrumbs as an `AppShell` prop | A root layout cannot know a page's trail (§ 8.1). |
| A submit button inside the portaled menu for "Sign out" | A mouse click unmounts the content before the submit runs when there is no exit animation (F18, § 8.5). |
| A development-server fixture | Next prefetches differently in development. The bug is a production prefetch (§ 10.3). |
| Wiring the shell into `paigasus-console` in this PR | That is SMA-511's scope, and the console has no auth yet (D2). |
| `prefetch`, `replace` and `scroll` props on `ZoneLink` | No consumer yet (§ 6.4). |
| A Tailwind sentinel class in this package now | It has no guard to feed until a console consumes the package. SMA-511 extends `ci/tailwind-source` (§ 2). |

---

## 13. Challenge log

The adversarial challenge (Opus, 2026-09-11) returned **NEEDS REWORK**: 2 blockers, 8 major
findings, 12 minor findings and 7 questions. This revision folds in the following.

| Finding | Change |
|---|---|
| B1 — the zone map has a null prototype, and React Flight throws for it | Confirmed in `runtime.ts:75, 253-256`. `getPublicConfig()` returns a plain object; `ZoneProvider` and `resolveZone` use `Object.hasOwn` (F17, § 6.1, § 9.1). |
| B2 — `next/link` adds the base path to the prefetch URL, so a `/gateway/*` filter watches the wrong URL | The allowlist rule; E3 expects `/iam/gateway/raw`; the controls sit on the E2 page after the links under test (F3, § 10.3). |
| M1 — a switcher item that renders `null` crashes Radix on keyboard open | The list components filter with `resolveZone` before they render a wrapper; a jsdom row with an unmatched first item (F18, § 8.3). |
| M2 — mouse sign-out depends on an exit animation | The form sits outside the portal; `requestSubmit()` from `onSelect`; E6 tests mouse and keyboard (§ 8.5). |
| M3 — a data-driven nav cannot be wrapped in `<Capability>` | `navStateOf(state, need?)`. The branch table moves into discovery's `capabilityOutcome`, which both `<Capability>` and `navStateOf` call, so there is one copy. (The challenger suggested a copy; a parity test for a copy cannot import `<Capability>` here, because the boundary rule bans it in tests too.) The discovery entry is `./client`, not `./disabled` (§ 7.1, § 9.2). |
| M4 — a root zone makes "unconfigured → nothing" impossible | `NavEntry.zone`, rule 1 on the declared zone; the root-zone limit for a bare `ZoneLink` is stated (§ 6.3, § 7.2). |
| M5 — a zone-relative href silently disappears | Partly: lists filter first, and a development warning. The throw is rejected (§ 12). |
| M6 — the two injection tests pass with no injection | A same-zone `@paigasus/ui` `Link` row in jsdom and in E4 (§ 10.2, § 10.3). |
| M7 — the Moon input lists are incomplete | Full lists for each task, with the upstream manifests and `vitest.config.ts` (§ 11.3). |
| M8 — the fixture's `.next` gets into other tasks | A separate fixture tsconfig, a negated input, and a plan measurement for `ts:lint`, `ts:fmt` and `next-public-free` (F20, § 11.6). |
| Minor — `RoleGrantRef` is not exported from `/client` | `SessionView['grants'][number]` (§ 5.3, § 7.2). |
| Minor — the degraded reason is read twice | The reason span is a sibling (§ 7.3). |
| Minor — "Account menu" does not contain the visible name | The visible text is the name (§ 8.5); the same rule for the switcher (§ 8.3). |
| Minor — the axe helper disables `region` | Enabled for the closed shells (§ 10.2). |
| Minor — href validation lets `\`, control characters and dot segments through | The `return-to.ts` rules plus a normalisation check (§ 6.2). |
| Minor — two entries can both be current | The longest match only (§ 7.4). |
| Minor — the boundary rule is a deny-list | The allowlist form (§ 9.1). |
| Minor — base-path uniqueness belongs in `zoneMapFromJson` | Added there; the `ZoneProvider` check stays for literal maps (§ 6.1, § 9.1). |
| Minor — an empty `nav` landmark | `PrimaryNav` returns `null` (§ 7.2). |
| Minor — `contracts->proto` "probably" changes | It changes (§ 11.4). |
| Minor — F9 wording; `_comment_exports`; import assertion | Corrected (F9, § 9.2). |
| Minor — a stale "not landed" note passes the liveness test | The test requires `'exists'` once the directory exists (§ 9.1). |
| Minor — no Tailwind sentinel for this package | **Rejected** for this PR; given to SMA-511 (§ 2, § 12). |
| Q — which page renders `PublicShell`; `returnTo` on "Sign in" | Answered (§ 6.5, § 8.2). |
| Q — does the fixture use `createNextConfig` | Yes; standalone `server.js` (§ 10.3). |
| Q — how the fixture imports the package | A plan measurement; E4 fails on two copies (§ 10.3). |
| Q — `prefetch`, `replace`, `scroll` on `ZoneLink` | No (§ 6.4, § 12). |
| Q — viewport prefetch for every nav entry | Next's default stays. A later issue can tune it with a measurement. |
| Q — refuse a same-zone `/auth/` href | It renders a plain `<a>` instead (§ 6.4, § 6.5). |
| Q — the failure scope of `ZoneLinkError` | Intended, and stated (§ 6.2). |
