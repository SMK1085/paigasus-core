// SPDX-License-Identifier: Apache-2.0
//
// A Server Action failed (spec § 6.1, third column). CLIENT component: a form renders it from its
// useActionState result. It shows the reason's copy when the form knows the reason, else the
// presentation's copy, and always the correlation id — never IAM's message.
'use client';

import type { ReactElement } from 'react';
import { usePathname } from 'next/navigation';
import { useZone } from '@paigasus/app-shell';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { formMessage } from './error-copy';
import { CorrelationReference, SignInAgain } from './error-reference';

export function FormError({ error }: { error: PaigasusError | null }): ReactElement | null {
  const pathname = usePathname();
  const { basePath } = useZone();
  if (error === null) return null;
  return (
    <div role="alert" data-testid="form-error" data-presentation={error.presentation} className="text-destructive mt-2 text-sm">
      <p>{formMessage(error)}</p>
      {/* usePathname() has no basePath (SMA-510 spec F19), and returnTo must keep it. */}
      {error.presentation === 'relogin' ? <SignInAgain returnTo={`${basePath}${pathname}`} /> : <CorrelationReference id={error.correlationId} />}
    </div>
  );
}
