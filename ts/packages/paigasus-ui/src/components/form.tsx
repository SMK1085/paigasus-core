// SPDX-License-Identifier: Apache-2.0
'use client';

import type { ComponentPropsWithoutRef, ReactElement, ReactNode } from 'react';
import { cloneElement, Fragment, isValidElement } from 'react';
import { Label as LabelPrimitive } from 'radix-ui';
import { cn } from '../lib/cn';

/*
 * Hand-written, not shadcn-generated — see dialog.tsx and the package README for why. Same
 * shape: `radix-ui`'s unified export, tokens from src/styles/tokens.css, no new dependency.
 */

export function Label({ className, ...props }: ComponentPropsWithoutRef<typeof LabelPrimitive.Root>): ReactElement {
  return <LabelPrimitive.Root className={cn('text-foreground peer-disabled:cursor-not-allowed peer-disabled:opacity-70 text-sm leading-none font-medium', className)} {...props} />;
}

export function Input({ className, ...props }: ComponentPropsWithoutRef<'input'>): ReactElement {
  return (
    <input
      className={cn(
        'border-input bg-background text-foreground placeholder:text-muted-foreground focus-visible:ring-ring flex h-9 w-full border px-3 py-1 text-sm shadow-sm transition-colors focus-visible:ring-1 focus-visible:outline-hidden disabled:cursor-not-allowed disabled:opacity-50 rounded-pgs',
        className,
      )}
      {...props}
    />
  );
}

export interface FieldProps {
  label: string;
  htmlFor: string;
  description?: string;
  error?: string;
  children: ReactNode;
}

/** Names the shape of an unwireable `children` value for the thrown error message below. */
function describeUnwireableChildren(children: ReactNode): string {
  if (children === null || children === undefined) {
    return 'no children';
  }
  if (Array.isArray(children)) {
    return 'an array of children';
  }
  if (typeof children === 'string' || typeof children === 'number') {
    return 'a plain text child';
  }
  if (isValidElement(children) && children.type === Fragment) {
    return 'a fragment';
  }
  return 'an unsupported child';
}

/**
 * Ties a label, an optional description, and an optional error to a single control by id —
 * that association, not the markup, is the point (SMA-503 task 11). `htmlFor` links the label;
 * `aria-describedby` links the description and error text; `aria-invalid` marks the control
 * invalid whenever `error` is set.
 *
 * `children` MUST be exactly one element — the control to wire. Zero children, an array of
 * children (a control plus an adjacent hint or icon is an ordinary thing to write, but Field
 * cannot tell which array member is "the control"), a raw string child, and a FRAGMENT (which
 * passes isValidElement but accepts only `key`, so every cloned prop would vanish) are all
 * REJECTED with a thrown error rather than silently skipping the wiring (SMA-503 fix round 1,
 * item 1; the fragment case is SMA-503 local review, finding F):
 * Field's entire reason to exist is that association, so an unwired control must never render
 * looking identical to a wired one. Wrap the control and any sibling in a single element (a
 * `<div>`, say) if more than one node is needed here.
 *
 * The control is a caller-supplied element (`Input` here, but the shape generalises), so the
 * only way to attach `aria-describedby`/`aria-invalid` to it without asking every consumer to
 * thread them through by hand is to clone it — a deliberate, narrow use of `cloneElement`:
 *
 * - `aria-describedby`: if the caller already put one on the child, it is PRESERVED and the
 *   generated description/error ids are appended to it, space-separated. Overwriting a
 *   caller-supplied describedby would silently drop whatever it pointed at — a real defect,
 *   not a stylistic choice.
 * - `aria-invalid`: fully OWNED by the `error` prop. An error message that does not also flip
 *   `aria-invalid` is a validity claim assistive technology cannot see, so `error` overrides
 *   any `aria-invalid` the caller set on the child.
 * - `id`: INJECTED as `htmlFor` when the child carries none (SMA-503 fix round 2, item 2).
 *   Without this the caller writes the same string twice — `<Field htmlFor="key-name">` and
 *   `<Input id="key-name">` — and a typo or a rename breaks the label association with no
 *   error, no failing test, and markup that still looks right.
 *
 *   A child that carries its OWN `id` is never overwritten: `id` is a document-global handle
 *   the caller may already be pointing something else at (a second `<label>`, an
 *   `aria-controls`, a `form` attribute, a test hook), and silently rewriting it would break
 *   whatever that is. But an id that DISAGREES with `htmlFor` is the very defect this
 *   injection exists to remove — the label points at nothing — so it is REJECTED with a thrown
 *   error rather than rendered. That is the same ruling, for the same reason, as the
 *   unwireable-`children` throw above: an unwired control must never render looking identical
 *   to a wired one. An id EQUAL to `htmlFor` is redundant but correct, and passes untouched.
 */
export function Field({ label, htmlFor, description, error, children }: FieldProps): ReactElement {
  /*
   * A FRAGMENT passes isValidElement (SMA-503 local review, finding F). Cloning one with
   * aria-describedby, aria-invalid and id drops all three in silence — a Fragment accepts only
   * `key` — so `<Field htmlFor="x"><><Input /></></Field>` would render a completely unwired
   * control that looks identical to a wired one. That is the exact defect the throw below
   * exists to prevent, reached through a different door, so it takes the same ruling.
   */
  if (!isValidElement(children) || children.type === Fragment) {
    throw new Error(
      `Field requires exactly one element child to wire aria-describedby/aria-invalid onto; received ${describeUnwireableChildren(children)}. Wrap the control (and any sibling) in a single element (a <div>, not a fragment) if more than one node is needed here.`,
    );
  }
  const child = children as ReactElement<Record<string, unknown>>;

  const childId = child.props['id'];
  if (childId !== undefined && childId !== htmlFor) {
    throw new Error(
      `Field htmlFor="${htmlFor}" does not match its control's own id ${JSON.stringify(childId)}; the label would point at nothing. Remove the id from the control (Field injects htmlFor as the id) or make the two agree.`,
    );
  }

  const descriptionId = description === undefined ? undefined : `${htmlFor}-description`;
  const errorId = error === undefined ? undefined : `${htmlFor}-error`;

  const existingDescribedBy = child.props['aria-describedby'];
  const describedBy = [typeof existingDescribedBy === 'string' ? existingDescribedBy : undefined, descriptionId, errorId].filter((part): part is string => Boolean(part)).join(' ');

  const injectedProps: Record<string, unknown> = {};
  if (childId === undefined) {
    injectedProps['id'] = htmlFor;
  }
  if (describedBy.length > 0) {
    injectedProps['aria-describedby'] = describedBy;
  }
  if (error !== undefined) {
    injectedProps['aria-invalid'] = true;
  }

  let control: ReactNode = child;
  if (Object.keys(injectedProps).length > 0) {
    /*
     * Field owns the association between its label/description/error and a caller-supplied
     * control it did not create, so cloning is the only way to attach the aria attributes
     * without asking every consumer to thread them through by hand — see the class doc above.
     */
    // eslint-disable-next-line @eslint-react/no-clone-element -- see the comment above: Field must attach aria-describedby/aria-invalid to a caller-supplied element it does not own.
    control = cloneElement(child, injectedProps);
  }

  return (
    <div className="flex flex-col gap-1.5">
      <Label htmlFor={htmlFor}>{label}</Label>
      {control}
      {description === undefined ? null : (
        <p id={descriptionId} className="text-muted-foreground text-sm">
          {description}
        </p>
      )}
      {error === undefined ? null : (
        <p id={errorId} className="text-destructive text-sm">
          {error}
        </p>
      )}
    </div>
  );
}
