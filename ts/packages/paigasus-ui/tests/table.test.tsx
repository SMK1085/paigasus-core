// SPDX-License-Identifier: Apache-2.0
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '../src/components/table';
import { expectNoAxeViolations } from './axe';

describe('Table', () => {
  it('renders in jsdom with no Next runtime', () => {
    render(
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Service</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow>
            <TableCell>iam</TableCell>
          </TableRow>
        </TableBody>
      </Table>,
    );
    expect(screen.getByRole('table')).toBeInTheDocument();
    expect(screen.getByRole('columnheader', { name: 'Service' })).toBeInTheDocument();
    expect(screen.getByRole('cell', { name: 'iam' })).toBeInTheDocument();
  });

  it('carries the AC-3 source sentinel on the table element', () => {
    render(<Table />);
    expect(screen.getByRole('table').className).toContain('[--paigasus-ui-source-probe:1]');
  });

  it('has no axe violations', async () => {
    render(
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Service</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow>
            <TableCell>iam</TableCell>
          </TableRow>
        </TableBody>
      </Table>,
    );
    await expectNoAxeViolations();
  });
});
