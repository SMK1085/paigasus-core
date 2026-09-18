// SPDX-License-Identifier: Apache-2.0
//
// The new API key's token (SMA-636 spec § 5.4). The section's result region renders it, OUTSIDE
// every row and panel that a revalidation re-renders (rule 3). The token lives in THIS component's
// useState and nowhere else in the browser: the section hands it over through `show()` the moment
// the issue call returns.
//
// It closes on "Done" and on `pagehide` ONLY (rule 6). It does not close on another submission, so
// key rotation works: issue, copy, then revoke the old key with the new token still on screen.
'use client';

import { useEffect, useImperativeHandle, useState, type ReactElement, type Ref } from 'react';
import { PRIMARY_BUTTON_CLASS, SECONDARY_BUTTON_CLASS } from '@paigasus/ui';

export type TokenPanelHandle = { show(token: string, prefix: string): void };

type Issued = { readonly token: string; readonly prefix: string };

export function TokenPanel({ ref }: { readonly ref: Ref<TokenPanelHandle> }): ReactElement | null {
  const [issued, setIssued] = useState<Issued | null>(null);
  const [copied, setCopied] = useState(false);

  useImperativeHandle(
    ref,
    () => ({
      show: (token: string, prefix: string) => {
        setIssued({ token, prefix });
        setCopied(false);
      },
    }),
    [],
  );

  useEffect(() => {
    if (issued === null) return undefined;
    const close = (): void => {
      setIssued(null);
    };
    window.addEventListener('pagehide', close);
    return () => {
      window.removeEventListener('pagehide', close);
    };
  }, [issued]);

  if (issued === null) return null;
  return (
    <div role="region" aria-label="New API key" data-testid="token-panel" className="border-input rounded-pgs flex flex-col gap-2 border p-3">
      <p className="text-sm">
        API key <code>{issued.prefix}</code> was issued. Copy the token now.
      </p>
      <code data-testid="token-value" className="text-sm break-all">
        {issued.token}
      </code>
      <p className="text-sm font-medium">You cannot see this token again.</p>
      <div className="flex flex-wrap gap-2">
        <button
          type="button"
          className={SECONDARY_BUTTON_CLASS}
          onClick={() => {
            navigator.clipboard.writeText(issued.token).then(
              () => {
                setCopied(true);
              },
              () => {
                setCopied(false);
              },
            );
          }}
        >
          Copy
        </button>
        <button
          type="button"
          className={PRIMARY_BUTTON_CLASS}
          onClick={() => {
            setIssued(null);
          }}
        >
          Done
        </button>
      </div>
      {copied ? (
        <p role="status" className="text-sm">
          Copied.
        </p>
      ) : null}
    </div>
  );
}
