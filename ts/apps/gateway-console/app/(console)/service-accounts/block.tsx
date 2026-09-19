// SPDX-License-Identifier: Apache-2.0
//
// The service-accounts section of both settings pages (SMA-636 spec § 4.1, § 4.2, § 4.6). A plain
// async FUNCTION, awaited by its page, not a JSX component: its error branch awaits SectionError,
// and a page must return a fully resolved tree (the reason app/_components/error-tail.tsx records).
//
// A forbidden list shows only this section's denial; the page stays 200. It passes the same five
// Server Actions on both pages: no action takes its owner from the page (plan SPEC DEVIATION 5).
//
// EVERY view kind renders inside ServiceAccountFrame, keyed by the owner PRN. The frame holds the
// result region and the TokenPanel. A successful issue revalidates (plan SPEC DEVIATION 8), and the
// revalidated section can be an error or a denial; the frame, and the token in it, stays through
// that (§ 5.4 rule 3). The owner PRN comes from the page, because an error or a denial has none.
import type { ReactElement } from 'react';
import { SectionError } from '../../_components/section-error';
import { ServiceAccountFrame } from '../../_components/service-account-frame';
import { ServiceAccountSection } from '../../_components/service-account-section';
import { allowModelCallsAction, archiveServiceAccountAction, createServiceAccountAction, issueApiKeyAction, revokeApiKeyAction } from './actions';
import type { OwnerKind, SectionView } from './view';

export async function serviceAccountsBlock({
  view,
  ownerKind,
  ownerPrn,
  path,
}: {
  readonly view: SectionView;
  readonly ownerKind: OwnerKind;
  readonly ownerPrn: string;
  readonly path: string;
}): Promise<ReactElement> {
  let body: ReactElement;
  if (view.kind === 'denied') {
    body = (
      <p data-testid="service-accounts-denied" className="text-muted-foreground text-sm">
        You cannot view service accounts here.
      </p>
    );
  } else if (view.kind === 'error') {
    body = await SectionError({ error: view.error });
  } else {
    body = (
      <ServiceAccountSection
        ownerKind={ownerKind}
        path={path}
        view={view}
        actions={{ create: createServiceAccountAction, allow: allowModelCallsAction, issue: issueApiKeyAction, revoke: revokeApiKeyAction, archive: archiveServiceAccountAction }}
      />
    );
  }
  return (
    <section aria-labelledby="service-accounts-heading" data-testid="service-accounts" className="flex flex-col gap-3">
      <h2 id="service-accounts-heading" className="text-lg font-semibold">
        Service accounts
      </h2>
      <ServiceAccountFrame key={ownerPrn} path={path} saOffset={view.kind === 'ok' ? view.saOffset : 0}>
        {body}
      </ServiceAccountFrame>
    </section>
  );
}
