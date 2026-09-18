// SPDX-License-Identifier: Apache-2.0
//
// "Your projects" on the overview (SMA-636 D16): the TEAM and PROJECT entries of myScopes(), a
// project as a link to its settings page. It reads the SAME myScopes() result as the organization
// switcher — a React cache() — so this list costs no IAM call. A project_admin cannot read the
// organization, so the organization page is a 403 for them; without this list they would reach a
// project page only by typing its URL. There is no team route (D4), so a team is a label.
//
// Server-safe and client-safe: its only runtime import is ZoneLink. Links do not prefetch (§ 4.1).
// At most the 50 scopes myScopes() shows (SCOPE_CAP) appear here.
import type { ReactElement } from 'react';
import { ZoneLink } from '@paigasus/app-shell';
import type { IamResult, MyScopes, ScopeEntry } from '@paigasus/console-core';
import { EmptyState } from '@paigasus/ui';

export type YourProjectRow = { readonly key: string; readonly kind: 'team' | 'project'; readonly label: string; readonly href: string | null };

export function yourProjectRows(scopes: IamResult<MyScopes>, basePath: string): YourProjectRow[] {
  if (!scopes.ok) return [];
  return scopes.value.entries.flatMap((entry: ScopeEntry): YourProjectRow[] => {
    if (entry.kind === 'team') return [{ key: entry.prn, kind: 'team', label: entry.label ?? entry.teamId, href: null }];
    if (entry.kind === 'project') return [{ key: entry.prn, kind: 'project', label: entry.label ?? entry.projectId, href: `${basePath}/orgs/${entry.orgId}/projects/${entry.projectId}` }];
    return [];
  });
}

function ListBody({ scopes, rows }: { readonly scopes: IamResult<MyScopes>; readonly rows: readonly YourProjectRow[] }): ReactElement {
  if (!scopes.ok) return <p className="text-muted-foreground text-sm">Your teams and projects could not be loaded.</p>;
  if (rows.length === 0) return <EmptyState title="No team or project scopes" />;
  return (
    <ul className="flex flex-col gap-1 text-sm">
      {rows.map((row) => (
        <li key={row.key} data-kind={row.kind}>
          {row.href === null ? (
            <span>{`Team: ${row.label}`}</span>
          ) : (
            <ZoneLink prefetch={false} href={row.href} className="hover:underline">
              {row.label}
            </ZoneLink>
          )}
        </li>
      ))}
    </ul>
  );
}

export function YourProjects({ scopes, basePath }: { readonly scopes: IamResult<MyScopes>; readonly basePath: string }): ReactElement {
  const rows = yourProjectRows(scopes, basePath);
  return (
    <section aria-labelledby="your-projects-heading" data-testid="your-projects" className="flex flex-col gap-2">
      <h2 id="your-projects-heading" className="text-lg font-semibold">
        Your projects
      </h2>
      <ListBody scopes={scopes} rows={rows} />
      {scopes.ok && scopes.value.hiddenCount > 0 ? <p className="text-muted-foreground text-xs">{`${String(scopes.value.hiddenCount)} more scopes are not shown.`}</p> : null}
    </section>
  );
}
