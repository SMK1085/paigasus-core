// SPDX-License-Identifier: Apache-2.0
//
// The new API key's token (SMA-636 spec § 5.4). The section's result region renders it, OUTSIDE
// every row and panel that a revalidation re-renders (rule 3). The token lives in THIS component's
// useState and nowhere else in the browser: the section hands it over through `show()` the moment
// the issue call returns.
//
// It closes on "Done" and on `pagehide` ONLY (rule 6). It does not close on another submission, so
// key rotation works: issue, copy, then revoke the old key with the new token still on screen.
//
// The `pagehide` close runs inside flushSync: a native `pagehide` listener gets React's
// DefaultEventPriority, so an ordinary setState schedules its render for a later macrotask. If the
// page enters the back/forward cache right after `pagehide`, the task queue freezes before that
// macrotask runs and the frozen DOM keeps showing the token. flushSync forces the removal to commit
// before this handler returns, so the token is gone from the DOM by the time the page can freeze.
// `onClosed` must run INSIDE that same flushSync call, not after it (SMA-636 fix round 2): it sets
// the parent frame's `tokenVisible` flag, and a call placed after flushSync is an ordinary update at
// default priority again, so the same freeze can leave "Issue key" disabled after a bfcache restore.
'use client';

import { useEffect, useImperativeHandle, useState, type ReactElement, type Ref } from 'react';
import { flushSync } from 'react-dom';
import { PRIMARY_BUTTON_CLASS, SECONDARY_BUTTON_CLASS } from '@paigasus/ui';

export type TokenPanelHandle = { show(token: string, prefix: string): void };

type Issued = { readonly token: string; readonly prefix: string };

export type TokenPanelProps = {
  readonly ref: Ref<TokenPanelHandle>;
  /**
   * Called whenever the panel clears its token: on "Done" and on `pagehide`. It is NOT called
   * from `show()` — the frame already knows a token is visible at that call site (Finding B, CR
   * round). A boolean only ever crosses this callback; the token itself never leaves this
   * component's own state.
   */
  readonly onClosed?: () => void;
};

export function TokenPanel({ ref, onClosed }: TokenPanelProps): ReactElement | null {
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
      // See the file comment above: a native pagehide handler must commit the removal before it
      // returns, or the back/forward cache can freeze the page with the token still in the DOM
      // (SMA-636 fix round 1).
      // eslint-disable-next-line @eslint-react/dom-no-flush-sync -- flushSync is the fix here, not the hazard.
      flushSync(() => {
        setIssued(null);
        onClosed?.();
      });
    };
    window.addEventListener('pagehide', close);
    return () => {
      window.removeEventListener('pagehide', close);
    };
  }, [issued, onClosed]);

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
            onClosed?.();
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
