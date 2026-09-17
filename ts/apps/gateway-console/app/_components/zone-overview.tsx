// SPDX-License-Identifier: Apache-2.0
//
// The zone overview (spec § 6). It is not a placeholder: it is the first exercise of ADR-0020
// discovery from a SECOND zone, and it is the control the follow-up issue's chat playground will
// gate on — the gateway answers a streaming request with a 400 carrying `param: "stream"` when the
// capability is off, so a console that could not see the capability would ship a control that
// always fails.
import type { ReactElement } from 'react';
import { EmptyState } from '@paigasus/ui';
import { STREAM_CAPABILITY, type GatewayView } from './gateway-state';

const STATE_COPY: Record<GatewayView['state'], { title: string; body: string }> = {
  available: { title: 'The gateway is available', body: 'It answered the capability probe.' },
  degraded: { title: 'The gateway is not available', body: 'It did not answer the capability probe. Try again in a moment.' },
  absent: { title: 'The gateway is not configured', body: 'No gateway entry is present in this deployment’s service map.' },
};

export function ZoneOverview({ view, scope }: { readonly view: GatewayView; readonly scope: { readonly orgId: string; readonly name: string } | null }): ReactElement {
  const copy = STATE_COPY[view.state];
  return (
    <section className="flex flex-col gap-6 p-8" data-testid="zone-overview">
      <div>
        <h1 className="text-2xl font-semibold">AI Gateway</h1>
        <p className="text-muted-foreground text-sm" data-testid="gateway-scope">
          {scope === null ? 'All organizations' : scope.name}
        </p>
      </div>

      <div data-testid="gateway-state" data-state={view.state}>
        <h2 className="text-lg font-semibold">{copy.title}</h2>
        <p className="text-muted-foreground text-sm">{copy.body}</p>
        {view.reason === null ? null : (
          <p className="text-muted-foreground mt-1 text-xs" data-testid="gateway-reason">
            Reason: <code>{view.reason}</code>
          </p>
        )}
      </div>

      {view.version === null ? null : (
        <p className="text-sm" data-testid="gateway-version">
          Version: <code>{view.version}</code>
        </p>
      )}

      <div>
        <h2 className="text-lg font-semibold">Capabilities</h2>
        {view.capabilities.length === 0 ? (
          <EmptyState title="No capabilities reported" />
        ) : (
          <ul className="text-sm" data-testid="gateway-capabilities">
            {view.capabilities.map((capability) => (
              <li key={capability}>
                <code>{capability}</code>
              </li>
            ))}
          </ul>
        )}
        <p className="text-muted-foreground mt-2 text-sm" data-testid="gateway-streaming" data-streaming={String(view.streaming)}>
          {view.streaming ? `Streaming chat (${STREAM_CAPABILITY}) is available.` : `Streaming chat (${STREAM_CAPABILITY}) is not available.`}
        </p>
      </div>

      <p className="text-muted-foreground text-sm" data-testid="playground-note">
        The chat playground is not part of this zone yet. It needs the gateway to authenticate an interactive user, which is tracked separately.
      </p>
    </section>
  );
}
