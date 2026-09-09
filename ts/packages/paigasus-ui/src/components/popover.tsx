// SPDX-License-Identifier: Apache-2.0
'use client';

import type { ComponentPropsWithoutRef, ReactElement } from 'react';
import { Popover as PopoverPrimitive } from 'radix-ui';
import { cn } from '../lib/cn';

/*
 * Hand-written, not shadcn-generated (SMA-503 task 10, controller ruling R15: the shadcn CLI
 * writes into a literal `./@/components/` directory this package has no path alias for, and
 * added a spurious `cn` npm dependency during task 9's trial run). Same shape as dialog.tsx and
 * dropdown-menu.tsx: `radix-ui`'s unified export, tokens from src/styles/tokens.css, no new
 * dependency.
 *
 * INTERNAL to this package. Combobox composes this with command.tsx; neither is re-exported
 * from src/index.ts.
 */

export const Popover = PopoverPrimitive.Root;
export const PopoverTrigger = PopoverPrimitive.Trigger;
export const PopoverAnchor = PopoverPrimitive.Anchor;

export function PopoverContent({ className, align = 'center', sideOffset = 4, ...props }: ComponentPropsWithoutRef<typeof PopoverPrimitive.Content>): ReactElement {
  return (
    <PopoverPrimitive.Portal>
      <PopoverPrimitive.Content
        align={align}
        sideOffset={sideOffset}
        className={cn(
          'bg-popover text-popover-foreground border-border data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 z-50 w-72 border p-0 shadow-md outline-hidden rounded-pgs',
          className,
        )}
        {...props}
      />
    </PopoverPrimitive.Portal>
  );
}
