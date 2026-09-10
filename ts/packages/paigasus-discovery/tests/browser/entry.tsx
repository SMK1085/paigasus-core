// SPDX-License-Identifier: Apache-2.0
//
// The browser-side half of app.tsx: hydrates the SAME tree fixture-server.ts rendered to static
// HTML, so the "before" and "after" DOM are identical and the only thing that changes between the
// two Playwright specs is whether this module ever ran at all.
import { hydrateRoot } from 'react-dom/client';
import { App } from './app.js';

const container = document.getElementById('root');
if (container === null) throw new Error('tests/browser/entry.tsx: #root is missing');
hydrateRoot(container, <App />);
