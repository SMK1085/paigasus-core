// SPDX-License-Identifier: Apache-2.0
//
// The browser-side half of app.tsx: hydrates the SAME tree fixture-server.ts rendered to real
// HTML, so the "before" and "after" DOM are identical and the only thing that changes between
// specs is whether this module ever ran at all.
import { hydrateRoot } from 'react-dom/client';
import { APPS, type AppVariant } from './app.js';

const container = document.getElementById('root');
if (container === null) throw new Error('tests/browser/entry.tsx: #root is missing');

// Set by fixture-server.ts's inline script, before this module loads — selects which of app.tsx's
// exported compositions to hydrate, so one entry point serves every SMA-509 finding-A variant
// (default, fragment child, non-forwarding custom-component child) with the same
// server-render/hydrate pairing fixture-server.ts used.
const variant = (window as Window & { __APP_VARIANT__?: AppVariant }).__APP_VARIANT__ ?? 'default';
const Component = APPS[variant];
hydrateRoot(container, <Component />);
