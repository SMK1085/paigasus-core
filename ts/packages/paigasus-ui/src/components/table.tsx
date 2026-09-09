// SPDX-License-Identifier: Apache-2.0
'use client';

import type { ComponentPropsWithoutRef, ReactElement } from 'react';
import { cn } from '../lib/cn';

/*
 * Sentinel A (SMA-503 spec §10.2). SOURCE_PROBE's value, below, is an arbitrary
 * custom-property utility that exists ONLY there, and it compiles to a single declaration in
 * the built CSS. ci/tailwind-source/run.mjs asserts it reaches a production console build,
 * which is what proves the app's Tailwind `@source` line still covers this package.
 *
 * It is a dedicated probe rather than a real style so that the assertion cannot pass for the
 * wrong reason. Do not remove it, do not rename it, and do not write its literal value
 * anywhere else in this file or under ts/apps/paigasus-console/ — Tailwind's scanner reads
 * raw file text, comments included, so a second copy (even in prose) would generate the
 * utility independently and silently disarm the gate. That is why this comment does not
 * spell out the literal itself; see the assignment below, or run.mjs's own PROBE_SOURCE
 * (assembled from string fragments for the same reason).
 */
const SOURCE_PROBE = '[--paigasus-ui-source-probe:1]';

export function Table({ className, ...props }: ComponentPropsWithoutRef<'table'>): ReactElement {
  return (
    <div className="relative w-full overflow-x-auto">
      <table className={cn(SOURCE_PROBE, 'w-full caption-bottom text-sm', className)} {...props} />
    </div>
  );
}

export function TableHeader({ className, ...props }: ComponentPropsWithoutRef<'thead'>): ReactElement {
  return <thead className={cn('[&_tr]:border-b', className)} {...props} />;
}

export function TableBody({ className, ...props }: ComponentPropsWithoutRef<'tbody'>): ReactElement {
  return <tbody className={cn('[&_tr:last-child]:border-0', className)} {...props} />;
}

export function TableRow({ className, ...props }: ComponentPropsWithoutRef<'tr'>): ReactElement {
  return <tr className={cn('border-border hover:bg-muted/50 border-b transition-colors', className)} {...props} />;
}

export function TableHead({ className, ...props }: ComponentPropsWithoutRef<'th'>): ReactElement {
  return <th className={cn('text-muted-foreground h-10 px-2 text-left align-middle font-medium', className)} {...props} />;
}

export function TableCell({ className, ...props }: ComponentPropsWithoutRef<'td'>): ReactElement {
  return <td className={cn('p-2 align-middle', className)} {...props} />;
}
