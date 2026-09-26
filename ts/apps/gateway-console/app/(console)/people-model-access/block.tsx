// SPDX-License-Identifier: Apache-2.0
//
// The "Model access for people" section of the organization page (SMA-676 spec § 4.4, D11). A plain
// async FUNCTION awaited by the page, like serviceAccountsBlock: its error branch awaits
// SectionError, and a page must return a fully resolved tree. A denial or an error shows only in this
// section; the page stays 200 (§ 5.1 step 2).
import type { ReactElement } from 'react';
import { PeopleModelAccessSection } from '../../_components/people-model-access-section';
import { SectionError } from '../../_components/section-error';
import { grantModelAccessAction, revokeModelAccessAction } from './actions';
import { PEOPLE_COPY, type PeopleModelAccessView } from './view';

export async function peopleModelAccessBlock({ view }: { readonly view: PeopleModelAccessView }): Promise<ReactElement> {
  let body: ReactElement;
  if (view.kind === 'disabled') {
    body = (
      <p data-testid="people-model-access-disabled" className="text-muted-foreground text-sm">
        {PEOPLE_COPY.disabled}
      </p>
    );
  } else if (view.kind === 'denied') {
    body = (
      <p data-testid="people-model-access-denied" className="text-muted-foreground text-sm">
        {PEOPLE_COPY.denied}
      </p>
    );
  } else if (view.kind === 'error') {
    body = await SectionError({ error: view.error });
  } else {
    body = <PeopleModelAccessSection view={view} actions={{ grant: grantModelAccessAction, revoke: revokeModelAccessAction }} />;
  }
  return (
    <section aria-labelledby="people-model-access-heading" data-testid="people-model-access" className="flex flex-col gap-3">
      <h2 id="people-model-access-heading" className="text-lg font-semibold">
        {PEOPLE_COPY.title}
      </h2>
      {body}
    </section>
  );
}
