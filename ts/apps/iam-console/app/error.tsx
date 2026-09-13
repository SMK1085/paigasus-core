// SPDX-License-Identifier: Apache-2.0
//
// The error boundary for errors in the ROUTE-GROUP LAYOUTS ((console)/layout.tsx, (public)/layout.tsx),
// which their own segment's error.tsx does not catch (spec § 6.1). It renders outside every
// provider, so it uses a plain <a>, not ZoneLink.
'use client';

import type { ReactElement } from 'react';
import { ErrorState } from '@paigasus/ui';
import { PRESENTATION_COPY } from './_components/error-copy';

export default function RootError({ error, reset }: { error: Error & { digest?: string }; reset: () => void }): ReactElement {
  return (
    <main className="p-8" data-testid="root-error">
      <ErrorState title={PRESENTATION_COPY.generic.title} description={PRESENTATION_COPY.generic.body} retry={reset} />
      {error.digest === undefined ? null : (
        <p className="text-muted-foreground mt-2 text-xs">
          Reference: <code>{error.digest}</code>
        </p>
      )}
      <a href="/iam/" className="mt-4 inline-block text-sm underline">
        Go to the start page
      </a>
    </main>
  );
}
