// SPDX-License-Identifier: Apache-2.0
//
// The last boundary: an error in the ROOT layout (spec § 6.1). It replaces the root layout, so it
// renders its own <html> and <body>, and globals.css may not have loaded — hence inline styles.
'use client';

import type { ReactElement } from 'react';

export default function GlobalError({ error }: { error: Error & { digest?: string } }): ReactElement {
  return (
    <html lang="en">
      <body style={{ fontFamily: 'system-ui, sans-serif', padding: '2rem' }}>
        <h1>Something went wrong</h1>
        <p>The console could not load. Try again in a moment.</p>
        {error.digest === undefined ? null : (
          <p>
            Reference: <code>{error.digest}</code>
          </p>
        )}
        <a href="/iam/">Go to the start page</a>
      </body>
    </html>
  );
}
