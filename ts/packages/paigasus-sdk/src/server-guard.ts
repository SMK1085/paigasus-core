// SPDX-License-Identifier: Apache-2.0
//
// The single `import 'server-only'` site for @paigasus/sdk (spec § 6.2). One site, so an entry
// point that forgets the guard is a visible omission rather than a copied line that drifted.
//
// What this buys, and what it does not. `server-only`'s exports map is
// { "react-server": "./empty.js", "default": "./index.js" }, and index.js is one unconditional
// throw — so a CLIENT component taking a VALUE import of a guarded entry fails the build. It does
// NOT cover the middleware/edge layer: Next sets the `react-server` condition there too, so this
// resolves to server-only's own empty.js and is a no-op (MEASURED in SMA-502). This package is not
// middleware-reachable by design and reads no environment, so no NEXT_RUNTIME check is added.
//
// AC 1 is enforced STRUCTURALLY and is not proven by a build: no test in this package runs a
// client-side Next build, so no test observes the throw. tests/server-guard.test.ts proves every
// guarded entry imports this module first; a console-side failing-build fixture is SMA-510's.
import 'server-only';
