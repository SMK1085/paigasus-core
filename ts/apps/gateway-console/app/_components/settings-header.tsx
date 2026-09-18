// SPDX-License-Identifier: Apache-2.0
//
// The header of the organization and project settings pages (SMA-636 spec § 4.1; controller F6:
// ONE copy, so the two pages cannot drift in shape; controller F17a: the status badge shows for
// every lifecycle, "Active" included, so it always names the node's own status, not only an
// exceptional one). `extra` is an optional line between the slug and the "Manage in IAM" link — the
// project page's team line is the only user of it today.
import type { ReactElement, ReactNode } from 'react';
import { ZoneLink } from '@paigasus/app-shell';

export function SettingsHeader({
  name,
  status,
  slug,
  extra,
  manage,
}: {
  readonly name: string;
  readonly status: string;
  readonly slug: string;
  readonly extra?: ReactNode;
  readonly manage: string | null;
}): ReactElement {
  return (
    <header className="flex flex-col gap-1">
      <div className="flex flex-wrap items-center gap-2">
        <h1 className="text-2xl font-semibold">{name}</h1>
        <span data-testid="node-status" className="border-input text-muted-foreground rounded-pgs border px-2 py-0.5 text-xs font-medium">
          {status}
        </span>
      </div>
      <p className="text-muted-foreground text-sm">{slug}</p>
      {extra}
      {manage === null ? null : (
        <ZoneLink href={manage} data-testid="manage-in-iam" className="text-sm underline">
          Manage in IAM
        </ZoneLink>
      )}
    </header>
  );
}
