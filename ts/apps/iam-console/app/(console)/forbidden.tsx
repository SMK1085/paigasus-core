// SPDX-License-Identifier: Apache-2.0
//
// The 403 view (spec § 6.2, AC 2). Next renders it, with HTTP 403, when a page under (console)
// calls forbidden() — PageError does that for a `forbidden` presentation. It renders INSIDE the
// (console) layout, so the shell stays (measured, spec § 13 #2). It needs
// experimental.authInterrupts (next.config.ts).
//
// It shows a fixed title, the request's correlation id and a way back. It never shows IAM's
// message. forbidden() takes no argument, so the id comes from the request header proxy.ts set;
// IAM adopted the same id, so it is the one in IAM's logs. FORBIDDEN_VIEW_CORRELATION records
// whether that route works (lib/correlation.ts); under 'fallback' the view shows no id.
import type { ReactElement } from 'react';
import { ZoneLink } from '@paigasus/app-shell';
import { ErrorState } from '@paigasus/ui';
import { FORBIDDEN_VIEW_CORRELATION, requestCorrelationId } from '../../lib/correlation';
import { PRESENTATION_COPY } from '../_components/error-copy';
import { CorrelationReference } from '../_components/error-reference';

export default async function Forbidden(): Promise<ReactElement> {
  const correlationId = FORBIDDEN_VIEW_CORRELATION === 'header' ? await requestCorrelationId() : null;
  return (
    <section className="p-8" data-testid="forbidden-view">
      <ErrorState title={PRESENTATION_COPY.forbidden.title} description={PRESENTATION_COPY.forbidden.body} />
      <CorrelationReference id={correlationId} />
      <ZoneLink href="/iam/orgs" className="mt-4 inline-block text-sm underline">
        Back to your organizations
      </ZoneLink>
    </section>
  );
}
