// SPDX-License-Identifier: Apache-2.0
//
// The error boundary for the console PAGES (spec § 6.1). An error in (console)/layout.tsx itself is
// caught one level up, by app/error.tsx. Next requires an error boundary to be a client component.
// It shows Next's digest as the reference, never the error message: a message can carry internal
// detail, and in production Next replaces it anyway.
'use client';

import type { ReactElement } from 'react';
import { ErrorState } from '@paigasus/ui';
import { PRESENTATION_COPY } from '../_components/error-copy';

export default function ConsoleError({ error, reset }: { error: Error & { digest?: string }; reset: () => void }): ReactElement {
  return (
    <section className="p-8" data-testid="console-error">
      <ErrorState title={PRESENTATION_COPY.generic.title} description={PRESENTATION_COPY.generic.body} retry={reset} />
      {error.digest === undefined ? null : (
        <p className="text-muted-foreground mt-2 text-xs">
          Reference: <code>{error.digest}</code>
        </p>
      )}
    </section>
  );
}
