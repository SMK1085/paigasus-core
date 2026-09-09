// SPDX-License-Identifier: Apache-2.0
'use client';

/*
 * `cmdk` depends on @radix-ui/react-dialog, @radix-ui/react-id, @radix-ui/react-primitive and
 * @radix-ui/react-compose-refs individually, while this package depends on the unified
 * `radix-ui`, which declares the same modules as ordinary dependencies of its own.
 *
 * TODAY THERE IS EXACTLY ONE COPY OF EACH. ts/pnpm-lock.yaml holds a single
 * @radix-ui/react-dialog@1.1.23 resolution and a single snapshot, shared by radix-ui@1.6.7,
 * cmdk@1.1.1 and @radix-ui/react-alert-dialog; `radix-ui` declares no bundledDependencies. An
 * earlier version of this comment said two copies already existed and labelled it MEASURED.
 * That was false (SMA-503 fix round 2, item 6).
 *
 * THE HAZARD IS LATENT. `radix-ui` pins its Radix deps EXACTLY ("@radix-ui/react-dialog":
 * "1.1.23"); `cmdk` uses a CARET ("^1.1.6"). They deduplicate only because the exact pin falls
 * inside the caret. A radix-ui bump past cmdk's range splits them into two copies, with no
 * install error and no warning.
 *
 * SO THE BAN STANDS: NOTHING HERE MAY COMPOSE ACROSS RADIX COPIES. A trigger from one copy
 * does not open content from the other, the failure is silent at runtime, and jsdom tests may
 * not surface it. cmdk's own Radix modules are reachable only through `Command.Dialog`, so
 * `Command.Dialog` IS BANNED. It costs nothing today and makes the split a non-event when it
 * arrives. Combobox composes radix-ui's Popover with cmdk's plain Command; Dialog comes from
 * radix-ui only. (SMA-503 spec §8)
 */

import type { ComponentPropsWithoutRef, ReactElement } from 'react';
import { Command as CommandPrimitive } from 'cmdk';
import { cn } from '../lib/cn';

/*
 * Hand-written, not shadcn-generated (SMA-503 task 10, controller ruling R15 — see popover.tsx
 * for why). This wraps only the four cmdk pieces Combobox actually needs; `CommandGroup` and
 * `CommandSeparator` are left out until a consumer needs them, and `Command.Dialog` is left out
 * on purpose (see the ban above).
 *
 * INTERNAL to this package. Not re-exported from src/index.ts.
 */

export function Command({ className, ...props }: ComponentPropsWithoutRef<typeof CommandPrimitive>): ReactElement {
  return <CommandPrimitive className={cn('bg-popover text-popover-foreground flex w-full flex-col overflow-hidden', className)} {...props} />;
}

export function CommandInput({ className, ...props }: ComponentPropsWithoutRef<typeof CommandPrimitive.Input>): ReactElement {
  return (
    <div className="border-border flex items-center border-b px-3">
      <CommandPrimitive.Input
        className={cn('placeholder:text-muted-foreground flex h-9 w-full rounded-pgs bg-transparent py-2 text-sm outline-hidden disabled:cursor-not-allowed disabled:opacity-50', className)}
        {...props}
      />
    </div>
  );
}

export function CommandList({ className, ...props }: ComponentPropsWithoutRef<typeof CommandPrimitive.List>): ReactElement {
  return <CommandPrimitive.List className={cn('max-h-[300px] overflow-x-hidden overflow-y-auto p-1', className)} {...props} />;
}

export function CommandEmpty({ className, ...props }: ComponentPropsWithoutRef<typeof CommandPrimitive.Empty>): ReactElement {
  return <CommandPrimitive.Empty className={cn('text-muted-foreground py-6 text-center text-sm', className)} {...props} />;
}

export function CommandItem({ className, ...props }: ComponentPropsWithoutRef<typeof CommandPrimitive.Item>): ReactElement {
  return (
    <CommandPrimitive.Item
      className={cn(
        'data-[selected=true]:bg-muted data-[selected=true]:text-foreground data-[disabled=true]:pointer-events-none data-[disabled=true]:opacity-50 relative flex cursor-default items-center gap-2 rounded-xs px-2 py-1.5 text-sm outline-hidden select-none',
        className,
      )}
      {...props}
    />
  );
}
