// SPDX-License-Identifier: Apache-2.0
//
// A Server Action failed (SMA-636 § 5.1). COPY of iam-console's app/_components/form-error.tsx
// (spec § 9; nothing gates a divergence), with ONE addition: `message` replaces the copy, for the
// two results whose words depend on the control (§ 5.3 "may already be allowed" and § 5.4 "…grant
// every role this account holds"). CLIENT component. It shows the reason's copy when the table
// knows the reason, else the presentation's copy, and always the correlation id — never IAM's
// message.
'use client';

import type { ReactElement } from 'react';
import { usePathname } from 'next/navigation';
import { useZone } from '@paigasus/app-shell';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { formMessage } from './error-copy';
import { CorrelationReference, SignInAgain } from './error-reference';

export function FormError({ error, message }: { readonly error: PaigasusError | null; readonly message?: string | undefined }): ReactElement | null {
  const pathname = usePathname();
  const { basePath } = useZone();
  if (error === null) return null;
  return (
    <div role="alert" data-testid="form-error" data-presentation={error.presentation} className="text-destructive mt-2 text-sm">
      <p>{message ?? formMessage(error)}</p>
      {/* usePathname() has no basePath (SMA-510 spec F19), and returnTo must keep it. */}
      {error.presentation === 'relogin' ? <SignInAgain returnTo={`${basePath}${pathname}`} /> : <CorrelationReference id={error.correlationId} />}
    </div>
  );
}
