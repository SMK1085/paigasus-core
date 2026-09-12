// SPDX-License-Identifier: Apache-2.0
//
// The Members section of the organization, team and project pages (spec § 5.2). A SERVER
// component: it passes the Server Actions to the client forms as props. `keep` holds the offset
// of the page's other list (for example `{ offset: 50 }`), so a member page link keeps it.
import type { ReactElement } from 'react';
import { EmptyState, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@paigasus/ui';
import { AttachMembershipForm, DetachMembershipButton } from '../../_components/membership-form';
import { Pager } from '../../_components/pager';
import { SectionError } from '../../_components/section-error';
import { attachMembershipAction, detachMembershipAction } from './actions';
import type { MembersData } from './members';

export type MembersSectionProps = {
  readonly nodePrn: string;
  readonly path: string;
  readonly data: MembersData;
  readonly keep?: Readonly<Record<string, number>>;
};

export function MembersSection({ nodePrn, path, data, keep = {} }: MembersSectionProps): ReactElement {
  return (
    <section aria-labelledby="members-heading" className="flex flex-col gap-3">
      <h2 id="members-heading" className="text-lg font-semibold">
        Members
      </h2>
      {data.list.ok ? (
        data.list.value.rows.length === 0 && data.list.value.offset === 0 ? (
          <EmptyState title="No members" />
        ) : (
          <>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Principal</TableHead>
                  {data.canDetach ? <TableHead>Action</TableHead> : null}
                </TableRow>
              </TableHeader>
              <TableBody>
                {data.list.value.rows.map((row) => (
                  <TableRow key={row.id}>
                    <TableCell>
                      <code className="text-xs">{row.principalPrn}</code>
                    </TableCell>
                    {data.canDetach ? (
                      <TableCell>
                        <DetachMembershipButton membershipId={row.id} principalPrn={row.principalPrn} action={detachMembershipAction} />
                      </TableCell>
                    ) : null}
                  </TableRow>
                ))}
              </TableBody>
            </Table>
            <Pager label="Member pages" path={path} param="moffset" offset={data.list.value.offset} nextOffset={data.list.value.nextOffset} keep={keep} />
          </>
        )
      ) : (
        <SectionError error={data.list.error} />
      )}
      {data.canAttach ? <AttachMembershipForm nodePrn={nodePrn} action={attachMembershipAction} /> : null}
    </section>
  );
}
