// SPDX-License-Identifier: Apache-2.0
//
// The gateway's state in one line (SMA-636 spec § 4.1: "the gateway state line, in compact form").
// The settings pages show it; the overview keeps the full view. Server-safe and client-safe.
import type { ReactElement } from 'react';
import type { GatewayView } from './gateway-state';

const LINE: Readonly<Record<GatewayView['state'], string>> = {
  available: 'The gateway is available.',
  degraded: 'The gateway is not available.',
  absent: 'The gateway is not configured.',
};

export function GatewayStateLine({ view }: { readonly view: GatewayView }): ReactElement {
  return (
    <p data-testid="gateway-state-line" data-state={view.state} className="text-muted-foreground text-sm">
      {LINE[view.state]}
    </p>
  );
}
