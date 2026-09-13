// SPDX-License-Identifier: Apache-2.0
//
// The root layout: html, body and the stylesheet ONLY (spec § 3.3). Each route group adds its own
// providers — the public group has no session, the console group has one.
import type { Metadata } from 'next';
import type { ReactElement, ReactNode } from 'react';
import './globals.css';

export const metadata: Metadata = { title: 'Paigasus IAM' };

export default function RootLayout({ children }: { children: ReactNode }): ReactElement {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
