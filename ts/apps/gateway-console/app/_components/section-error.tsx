// SPDX-License-Identifier: Apache-2.0
//
// COPY of ts/apps/iam-console/app/_components/section-error.tsx (SMA-636 spec § 9; nothing gates a
// divergence). A SECTION read failed: the service accounts or the Projects list of a settings page.
// SERVER component. It NEVER throws: the rest of the page still renders. Its callers AWAIT it as a
// function (see error-tail.tsx), so a page returns a fully resolved tree.
import type { ReactElement } from 'react';
import type { PaigasusError } from '@paigasus/sdk/errors';
import { EmptyState, ErrorState } from '@paigasus/ui';
import { errorTail } from './error-tail';
import { PRESENTATION_COPY } from './error-copy';

export async function SectionError({ error }: { error: PaigasusError }): Promise<ReactElement> {
  const copy = PRESENTATION_COPY[error.presentation];
  if (error.presentation === 'disabled' || error.presentation === 'not-found') {
    return (
      <div data-testid="section-error" data-presentation={error.presentation}>
        <EmptyState title={copy.title} description={copy.body} />
      </div>
    );
  }
  return (
    <div data-testid="section-error" data-presentation={error.presentation}>
      <ErrorState title={copy.title} description={copy.body} />
      {await errorTail(error)}
    </div>
  );
}
