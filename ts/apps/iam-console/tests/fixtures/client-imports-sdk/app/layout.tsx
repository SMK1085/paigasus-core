// SPDX-License-Identifier: Apache-2.0
import type { ReactElement, ReactNode } from 'react';

export default function RootLayout({ children }: { children: ReactNode }): ReactElement {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
