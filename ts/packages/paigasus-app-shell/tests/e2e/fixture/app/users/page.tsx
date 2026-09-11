// SPDX-License-Identifier: Apache-2.0
import type { ReactElement } from 'react';
import { PathnameProbe } from './pathname-probe';

export default function UsersPage(): ReactElement {
  return (
    <main>
      <h1>Users</h1>
      <PathnameProbe />
    </main>
  );
}
