// SPDX-License-Identifier: Apache-2.0
'use client';

import type { ReactElement } from 'react';
import { TenancyService, createIamClient } from '@paigasus/sdk/iam';

// A VALUE import of a guarded SDK entry. In a client component this must fail the build.
export function SdkUser(): ReactElement {
  return <p>{`${typeof createIamClient} ${TenancyService.typeName}`}</p>;
}
