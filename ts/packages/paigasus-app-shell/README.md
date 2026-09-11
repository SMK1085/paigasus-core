# @paigasus/app-shell

The shared chrome of every Paigasus console zone: the header, the primary
navigation, the org and team switcher, the user menu, and breadcrumbs.

The package is also the one place that knows the zone map. A link to another
zone must be a hard navigation (ADR-0017). `ZoneLink` makes it one.

## Providers

| Component                             | Needs                                                                                                                                        |
| ------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `AppShell`                            | `ZoneProvider` and `SessionProvider` (from `@paigasus/auth/client`). Without a `SessionProvider` it throws `UseSessionOutsideProviderError`. |
| `PublicShell`                         | `ZoneProvider` only. It never calls `useSession()`.                                                                                          |
| `PrimaryNav`                          | `ZoneProvider` and `SessionProvider`.                                                                                                        |
| `ZoneLink`, `Switcher`, `Breadcrumbs` | `ZoneProvider`.                                                                                                                              |

The app's server layout calls `getPublicConfig()` from
`@paigasus/next-config/runtime` and passes `zone` and `zones` to `ZoneProvider`
as props. No other value crosses to the browser. `getPublicConfig()` returns a
plain object, so read the map with `Object.hasOwn`, never with
`zones[id] === undefined`.

There is no default zone map. `useZone()` with no `ZoneProvider` throws
`ZoneProviderMissingError`. A default map would make every cross-zone link look
same-zone.

`ZoneProvider` throws `ZoneConfigError` when the current zone is not in the map,
or when two zones have the same base path.

The app decides which shell to render. It calls `getSession()` in its server
layout. With a session it renders `AppShell` inside `SessionProvider`. With no
session it renders `PublicShell`.

## The href contract

Write every href as the ingress sees it: `/gateway/usage`,
`/iam/users?page=2#top`. The href must be a path that starts with exactly one
`/`. `resolveZone` throws `ZoneLinkError` for every other form: an absolute URL,
a protocol-relative URL, a backslash, a control character or whitespace, a
relative path, an empty string, and a path that URL normalisation changes (a dot
segment, or a character that the browser re-encodes). The error message never
contains the href, because an href can carry user data.

| Resolution                | `ZoneLink` renders                                                           |
| ------------------------- | ---------------------------------------------------------------------------- |
| no zone matches           | nothing, plus one development warning that names only the first path segment |
| same zone, under `/auth/` | a plain `<a>` with the full href                                             |
| same zone                 | `next/link` with the zone-relative remainder. Next adds the base path back.  |
| another zone              | a plain `<a>` with the full href: a hard navigation                          |

"No match renders nothing" is a normal case: an IAM-only deployment has no
gateway zone, and its gateway links must disappear. The development warning
shows the other cause, a zone-relative href such as `/users`.

### Inject `ZoneLink` into `@paigasus/ui`

```tsx
'use client';
import { ZoneLink } from '@paigasus/app-shell';
import { LinkProvider } from '@paigasus/ui';

<LinkProvider link={ZoneLink}>{children}</LinkProvider>;
```

Then every `@paigasus/ui` link is zone-safe too. The consequence: under this
injection, **every `@paigasus/ui` href must also be a full path**.

### The root-zone limit

When a zone is mounted at the origin root (base path `''`), every href resolves
to some zone. A bare `ZoneLink` then cannot detect an unconfigured zone:
`/billing/x` resolves to the root zone. `PrimaryNav` closes this for nav
entries, because each entry declares its `zone`. A bare `ZoneLink` in a
deployment with a root zone cannot.

## Auth links are never soft

Login and logout are route handlers, not pages. A `next/link` to them starts an
RSC fetch, and login rejects a request whose `Sec-Fetch-Mode` is not `navigate`.

- "Sign in" (`PublicShell`) is a plain `<a href="${basePath}/auth/login">`. It
  carries no `returnTo`: the middleware redirect adds one for a protected page.
- "Sign out" (`UserMenu`) submits a native `POST` form to
  `${basePath}/auth/logout`. The form sits outside the menu portal, so it stays
  mounted when a mouse click closes the menu.
- `ZoneLink` renders a plain `<a>` for any same-zone href under `/auth/`.

## Navigation and the three service states

The app resolves each entry's `ServiceState` on the server (for example with
`discovery.getServiceState('iam', token)`) and passes `navStateOf(state, need)`
as the entry's `state`. `navStateOf` applies `@paigasus/discovery`'s
`capabilityOutcome`, which is the one copy of `<Capability>`'s branch table.
`navStateOf` has no `'use client'` directive, so a server layout can call it.

For each entry, the first rule that applies decides:

1. The entry's `zone` is not in the zone map: nothing.
2. The entry's `href` does not resolve to its `zone`: `ZoneLinkError`.
3. The state is `absent`: nothing.
4. `requires` is set and `can(session, requires)` is false: nothing. This is
   cosmetic only, and `can()` fails open when grants are not available.
5. The state is `degraded`: a disabled entry with a visible reason.
6. The state is `available`: a `ZoneLink`. Only the longest matching entry gets
   `aria-current="page"`.

The degraded entry has no `href` and no `<a>`. It is `role="link"`,
`aria-disabled="true"` and focusable. Its reason text is visible and is also its
accessible description.

## Styling: the `@source` obligation

The components use Tailwind utility classes on `@paigasus/ui`'s design tokens.
Tailwind's scan root is the app's own directory, so **every consuming app needs
its own `@source` line** that covers `ts/packages/paigasus-app-shell/src`. The
`@paigasus/ui` README states the same rule. Without the line, the classes
disappear only in a production build. SMA-511 adds the line to the console.

## Import specifiers

Every relative import in `src/` has no file extension. Turbopack in Next 16.3.4
does not map `./x.js` to `./x.ts`, and a consuming Next app compiles every file
here. `tests/structure/source-shape.test.ts` fails on a `.js` relative value
import.

## Tests

| Task       | What it runs                                                                                                                                                                                                                                                                                                                                                  |
| ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `test`     | vitest in jsdom: the resolver, every component, axe on every shell state, and a keyboard script for the switcher.                                                                                                                                                                                                                                             |
| `test-e2e` | Playwright in Chromium against a production build of the Next app in `tests/e2e/fixture/`. It proves that no RSC request leaves a page's own allowlist, that a raw `next/link` to another zone is seen (a negative control), that a same-zone click stays soft, that a degraded entry never navigates, and that sign-out posts once by mouse and by keyboard. |

## What the tests do not prove

- **A real second zone.** The browser tier proves that the IAM zone sends no RSC
  request outside its own targets. It does not run a second Next app. The failure
  that ADR-0017 describes needs two apps behind one ingress: that is SMA-513.
- **Colour contrast.** jsdom axe cannot compute it, and the fixture has no
  Tailwind build.
- **The Flight handoff in an app.** `getPublicConfig()`'s test proves that it
  returns a plain object. The fixture uses a literal map, so no test here sends
  the real map through React Flight. SMA-511's app does.
- **The app's composition.** No test proves that an app passes `navStateOf()`
  output and not a raw `ServiceState`. The type accepts only `NavEntryState`, but
  a wider object can still pass. SMA-511 owns the composition.
