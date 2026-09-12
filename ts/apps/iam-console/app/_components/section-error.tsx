// SPDX-License-Identifier: Apache-2.0
//
// A SECTION read failed (spec § 6.1, second column): one part of a page, for example "All
// organizations" or a member list. SERVER component. It NEVER throws: the rest of the page still
// renders. A forbidden section shows the 403 inline, WITH the correlation id, because the
// PaigasusError is right here (unlike the page-level view, spec § 6.2).
import type { ReactElement } from 'react';
import type { PaigasusError } from '@paigasus/sdk/errors';
import { EmptyState, ErrorState } from '@paigasus/ui';
import { requestPath } from '../../lib/correlation';
import { PRESENTATION_COPY } from './error-copy';
import { CorrelationReference, SignInAgain } from './error-reference';

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
      {error.presentation === 'relogin' ? <SignInAgain returnTo={await requestPath()} /> : <CorrelationReference id={error.correlationId} />}
    </div>
  );
}
