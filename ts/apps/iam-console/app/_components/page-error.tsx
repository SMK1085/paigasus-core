// SPDX-License-Identifier: Apache-2.0
//
// A PAGE read failed (spec § 6.1, first column). SERVER component.
//
//   forbidden  -> forbidden(): Next renders (console)/forbidden.tsx with HTTP 403 (AC 2)
//   not-found  -> notFound()
//   disabled   -> EmptyState "This feature is not enabled on this IAM"
//   relogin    -> ErrorState + "Sign in again" (a link, never a redirect: spec § 6.4)
//   the rest   -> ErrorState + the correlation id
//
// forbidden() and notFound() are called FIRST, before any await, so they throw before the page has
// rendered anything. Do not add a loading.tsx under (console): its Suspense boundary would let
// Next commit a 200 before the throw, and the 403 status would be lost.
import type { ReactElement } from 'react';
import { forbidden, notFound } from 'next/navigation';
import type { PaigasusError } from '@paigasus/sdk/errors';
import { EmptyState, ErrorState } from '@paigasus/ui';
import { requestPath } from '../../lib/correlation';
import { PRESENTATION_COPY } from './error-copy';
import { CorrelationReference, SignInAgain } from './error-reference';

export async function PageError({ error }: { error: PaigasusError }): Promise<ReactElement> {
  if (error.presentation === 'forbidden') forbidden();
  if (error.presentation === 'not-found') notFound();
  const copy = PRESENTATION_COPY[error.presentation];
  if (error.presentation === 'disabled') {
    return (
      <section className="p-8" data-testid="page-error" data-presentation={error.presentation}>
        <EmptyState title={copy.title} description={copy.body} />
      </section>
    );
  }
  return (
    <section className="p-8" data-testid="page-error" data-presentation={error.presentation}>
      <ErrorState title={copy.title} description={copy.body} />
      {error.presentation === 'relogin' ? <SignInAgain returnTo={await requestPath()} /> : <CorrelationReference id={error.correlationId} />}
    </section>
  );
}
