'use client';
// SPDX-License-Identifier: Apache-2.0
//
// A CLIENT component, and it has to be. <Capability> is an async server component and cannot
// attach event handlers, but blocking activation requires them: `tabindex` is NOT inherited, so
// putting tabIndex={-1} on a wrapper leaves a nested <a> or <button> fully keyboard-activatable
// while `aria-disabled` makes every attribute assertion pass. Capture-phase handlers are what
// actually disable the subtree.
//
// It carries NO Tailwind utility classes. Tailwind v4's scan root is the working directory and
// Moon runs `next build` from the app's own directory, so a package shipping utility classes needs
// an `@source` line in every consumer — and forgetting it drops the classes silently, ONLY in a
// production build. Everything cosmetic is exposed as a data attribute for the consumer to style.

import { useId, type KeyboardEvent, type MouseEvent, type ReactElement, type ReactNode } from 'react';
import type { DegradedReason } from './types.js';

const REASON_TEXT: Readonly<Record<DegradedReason, string>> = {
  timeout: 'not answering (timed out)',
  network: 'not answering (unreachable)',
  unauthorized: 'not answering (your session was rejected)',
  'not-implemented': 'not answering (this build does not provide it)',
  'bad-response': 'not answering (unrecognised response)',
  'server-error': 'not answering (server error)',
  'cache-unavailable': 'not answering (discovery cache unavailable)',
};

export function reasonText(service: string, reason: DegradedReason): string {
  return `${service} is ${REASON_TEXT[reason]}`;
}

export type CapabilityDisabledProps = {
  readonly service: string;
  readonly reason: DegradedReason;
  readonly children: ReactNode;
};

export function CapabilityDisabled({ service, reason, children }: CapabilityDisabledProps): ReactElement {
  // useId, never a hardcoded id: two <Capability> elements on one page would otherwise emit
  // duplicate ids and break the aria-describedby association for both.
  const describedBy = useId();

  const block = (event: MouseEvent | KeyboardEvent): void => {
    event.preventDefault();
    event.stopPropagation();
  };

  return (
    <span
      data-capability-state="degraded"
      data-capability-reason={reason}
      aria-disabled="true"
      aria-describedby={describedBy}
      style={{ pointerEvents: 'none' }}
      onClickCapture={block}
      onKeyDownCapture={(event) => {
        if (event.key === 'Enter' || event.key === ' ') block(event);
      }}
    >
      {children}
      <span
        id={describedBy}
        style={{
          position: 'absolute',
          width: 1,
          height: 1,
          padding: 0,
          margin: -1,
          overflow: 'hidden',
          clip: 'rect(0 0 0 0)',
          whiteSpace: 'nowrap',
          border: 0,
        }}
      >
        {reasonText(service, reason)}
      </span>
    </span>
  );
}
