// SPDX-License-Identifier: Apache-2.0
//
// The public group: the signed-out landing page. A ZoneProvider and NO SessionProvider (PublicShell
// never calls useSession(); SMA-510 spec § 8.2).
//
// `await connection()` opts the group out of prerendering. getPublicConfig() reads the deployment's
// environment, which does not exist during `next build`.
import type { ReactElement, ReactNode } from 'react';
import { connection } from 'next/server';
import { getPublicConfig } from '../../lib/config';
import { Providers } from '../providers';

export default async function PublicLayout({ children }: { children: ReactNode }): Promise<ReactElement> {
  await connection();
  const { zone, zones } = getPublicConfig();
  return (
    <Providers zone={zone} zones={zones} session={null}>
      {children}
    </Providers>
  );
}
