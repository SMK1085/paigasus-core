// SPDX-License-Identifier: Apache-2.0
'use client';

import type { ReactElement } from 'react';
import { useState } from 'react';
import { Command, CommandEmpty, CommandInput, CommandItem, CommandList } from './command';
import { Popover, PopoverAnchor, PopoverContent } from './popover';

/*
 * Hand-written, not shadcn-generated (SMA-503 task 10, controller ruling R15 — see popover.tsx
 * for why). The shadcn combobox recipe is a copy-paste snippet the consumer assembles from
 * Popover + Command; this is a single self-contained component taking `items` instead.
 *
 * This composes radix-ui's Popover with cmdk's plain `Command` — see command.tsx's header for
 * why `Command.Dialog` is banned and never used here. (The ban guards a LATENT split between
 * radix-ui's exact Radix pins and cmdk's caret ranges; the two deduplicate to one copy today.)
 *
 * cmdk's `CommandInput` already renders its underlying `<input>` with `role="combobox"`,
 * `aria-expanded`, `aria-controls` (pointing at the `CommandList`'s `role="listbox"`) and
 * `aria-activedescendant` built in, so that one input doubles as both the popover's anchor and
 * the filter text box — there is no separate trigger button.
 */

export interface ComboboxItem {
  value: string;
  label: string;
}

export interface ComboboxProps {
  items: ComboboxItem[];
  value?: string;
  onValueChange?: (value: string) => void;
  placeholder?: string;
  emptyMessage?: string;
  /**
   * Merged onto the outermost rendered element. `Popover` (Radix's `Popover.Root`) renders no
   * DOM node of its own, so the outermost element is `Command` — which runs the `cn` merge
   * against its own base classes, exactly like every other component in this package.
   */
  className?: string;
}

/**
 * A single-select combobox.
 *
 * CONTROLLED AND UNCONTROLLED, and the displayed text follows the value in both (SMA-503 fix
 * round 2, item 1). The input shows a DRAFT while the user is filtering and the selected item's
 * label the rest of the time; `draft === null` means "show the label of the current value".
 * Three consequences, and each is the fix for a real defect the previous `useState(label)`
 * initializer had:
 *
 * - A controlled parent that changes `value` programmatically — restoring from a URL, resetting
 *   a form, clearing after a failed save — repaints the input, because the label is DERIVED per
 *   render rather than captured once by a `useState` initializer.
 * - A controlled parent that DECLINES a selection never gets the rejected label painted in:
 *   `onSelect` only clears the draft, so the display falls back to the label of the value the
 *   parent still holds. The component never writes the caller's value for it.
 * - A programmatic change that lands WHILE the user has typed a draft still wins, via the
 *   render-time adjustment below (React's documented "adjust state when a prop changes"
 *   pattern — cheaper and flash-free compared with an effect).
 *
 * Uncontrolled use is unchanged: `internalValue` holds the selection, so selecting an item still
 * paints its label and the component remains usable with no `value`/`onValueChange` at all.
 */
export function Combobox({ items, value, onValueChange, placeholder, emptyMessage = 'No results found.', className }: ComboboxProps): ReactElement {
  const [open, setOpen] = useState(false);
  const [internalValue, setInternalValue] = useState(value);
  const currentValue = value === undefined ? internalValue : value;
  const selectedLabel = items.find((item) => item.value === currentValue)?.label ?? '';

  const [draft, setDraft] = useState<string | null>(null);
  const [syncedValue, setSyncedValue] = useState(currentValue);
  if (currentValue !== syncedValue) {
    // The value moved underneath us (a controlled parent, or our own uncontrolled selection).
    // Drop the draft so the input shows the new selection rather than a stale filter string.
    setSyncedValue(currentValue);
    setDraft(null);
  }

  const search = draft ?? selectedLabel;

  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) {
          // The popover was dismissed without a selection (Escape, an outside click). The
          // draft is an abandoned filter string, so drop it and let the input fall back to the
          // label of the value actually held — otherwise the box keeps showing text that
          // matches nothing the component selected (SMA-503 local review, finding C).
          setDraft(null);
        }
      }}
    >
      {/*
       * B (SMA-503 local review): filter ONLY while the user is typing. `search` falls back to
       * `selectedLabel` when there is no draft, so an unconditional `shouldFilter` would make
       * cmdk score every item against the CURRENT selection's label on reopen and narrow the
       * list to the one item already chosen — the user could not change their mind without
       * clearing the box first. `draft !== null` is exactly "the user is typing".
       */}
      <Command shouldFilter={draft !== null} className={className}>
        <PopoverAnchor asChild>
          <CommandInput
            value={search}
            placeholder={placeholder}
            onValueChange={(next) => {
              setDraft(next);
              setOpen(true);
            }}
            onFocus={() => {
              setOpen(true);
            }}
          />
        </PopoverAnchor>
        <PopoverContent
          align="start"
          onInteractOutside={(event) => {
            /*
             * `PopoverContent` is PORTALLED, so the anchor — our own `CommandInput` — counts as
             * OUTSIDE it, and a pointer-down on the very input that drives the list would close
             * the popover. That was harmless until the draft started being cleared on close
             * (finding C): the close then discarded what the user had typed, and the next
             * keystroke appended to the restored label instead ('IAM' + 'gate' = 'IAMgate',
             * MEASURED against two tests). Keep the popover open for interactions inside the
             * Command root, so a close means a REAL dismissal — Escape, or a click elsewhere.
             *
             * `[cmdk-root]` is cmdk's own marker attribute on the `Command` element. This file
             * already reasons about cmdk internals (see the `onSelect` comment below), and the
             * portal means the root's subtree holds only the anchor, never the list.
             */
            const target = event.target;
            if (target instanceof Element && target.closest('[cmdk-root]') !== null) {
              event.preventDefault();
            }
          }}
          onOpenAutoFocus={(event) => {
            // The CommandInput already holds focus (that's what opened the popover); Radix's
            // default auto-focus would move it onto the content div and blur the input mid-type.
            event.preventDefault();
          }}
        >
          <CommandList>
            <CommandEmpty>{emptyMessage}</CommandEmpty>
            {items.map((item) => (
              <CommandItem
                key={item.value}
                value={item.value}
                keywords={[item.label]}
                onSelect={(selected) => {
                  /*
                   * cmdk NORMALISES the value it hands back, so `selected` is not necessarily
                   * any `ComboboxItem.value`. MEASURED against the pinned cmdk 1.1.1
                   * (`useValue` in dist/index.mjs): the stored value is `value.trim()` — it is
                   * TRIMMED, and NOT lower-cased. The only `toLowerCase` in the package is
                   * inside command-score, which scores a match and never touches the stored
                   * value. So `"gateway "` comes back as `"gateway"`, while `"IAM"` comes back
                   * unchanged. A trimmed callback argument matches no item, `selectedLabel`
                   * resolves to '' and the input blanks itself.
                   *
                   * Resolve back to the RAW value and report that. Exact first, so an item
                   * whose value needs no normalisation is unaffected; the trimmed comparison
                   * is the fallback. If nothing resolves — a cmdk version that normalises some
                   * other way — pass cmdk's string through rather than swallowing the event.
                   */
                  const resolved = items.find((candidate) => candidate.value === selected) ?? items.find((candidate) => candidate.value.trim() === selected);
                  const rawValue = resolved?.value ?? selected;
                  if (value === undefined) {
                    setInternalValue(rawValue);
                  }
                  // Deliberately NOT `setDraft(item.label)`: in controlled use the parent owns
                  // the value, so painting the label here would show a change the parent may
                  // reject. Clearing the draft falls back to the label of whatever value the
                  // parent ends up holding.
                  setDraft(null);
                  onValueChange?.(rawValue);
                  setOpen(false);
                }}
              >
                {item.label}
              </CommandItem>
            ))}
          </CommandList>
        </PopoverContent>
      </Command>
    </Popover>
  );
}
