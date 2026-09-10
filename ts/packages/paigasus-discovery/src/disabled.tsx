'use client';
// SPDX-License-Identifier: Apache-2.0
//
// A CLIENT component, and it has to be. <Capability> is an async server component and cannot
// attach event handlers, but blocking activation requires them: `tabindex` is NOT inherited, so
// putting tabIndex={-1} on a wrapper leaves a nested <a> or <button> fully keyboard-activatable
// while `aria-disabled` makes every attribute assertion pass. Capture-phase handlers are what
// actually disable the subtree.
//
// `pointer-events: none` lives on the CHILD, not the wrapper. Two failure modes rejected an
// earlier design that put it on the wrapper instead:
//
// - With `pointer-events: none` on the WRAPPER: when this component sits inside a clickable
//   ancestor (a nav row whose whole area navigates, say), the browser's hit-testing skips a
//   pointer-events:none element entirely and targets whatever is underneath — here, straight
//   through to the ancestor. The wrapper is then absent from the event path, `onClickCapture`
//   never fires on it, and the ancestor still activates.
// - Removing it entirely: the capture handlers below then work in every composition, but this
//   is a CLIENT component — before hydration there are no event handlers at all, while a CSS
//   rule still applies with no JavaScript. That opens a window where a disabled item is fully
//   clickable until React attaches.
//
// Putting `pointer-events: none` on the CHILD instead keeps the wrapper interactive, so a click
// still lands ON the wrapper (the child cannot be hit) and the capture handler still blocks it
// before it reaches any ancestor — while the CSS alone, with no JavaScript needed, keeps the
// child inert before hydration too.
//
// SMA-509 finding A: an earlier version reached the child only by `cloneElement`-ing a merged
// `style` onto it, which works only when `children` is a single plain-element node that reads
// `props.style` and forwards it to its own DOM node. `isValidElement` also accepts a
// `React.Fragment` (which ignores `style` entirely — it renders no DOM node of its own) and a
// custom component (which forwards `style` only if its author chose to), so
// `<Capability><NavLink href="/audit">Audit</NavLink></Capability>` could stay fully interactive
// before hydration, with no capture handlers yet attached — the exact guarantee this design
// exists to give.
//
// The fix is a scoped CSS rule, not a better clone: `[data-capability-state='degraded'] > *`,
// keyed on the `data-capability-state` attribute the wrapper already sets and the README already
// documents as the styling contract. A Fragment's children and a custom component's rendered
// root both land as direct children of the wrapper in the DOM however `children` is shaped, so
// `> *` reaches every case with no JavaScript needed — including before hydration. It is emitted
// as a `<style>` element inside the wrapper, so the package still ships no separate CSS asset for
// a consumer to wire up.
//
// The `cloneElement` below still runs, for two reasons unrelated to pointer-events: it is still
// how the wrapper attaches `aria-disabled`/`aria-describedby` to the child itself (see the
// comment further down), and its own `pointerEvents: 'none'` stays as belt-and-braces — an
// existing jsdom test asserts the inline style directly (`link.style.pointerEvents`), which the
// CSS rule alone cannot satisfy, since jsdom's `element.style` reflects only the `style=""`
// attribute, never a matched stylesheet rule.
//
// It carries NO Tailwind utility classes. Tailwind v4's scan root is the working directory and
// Moon runs `next build` from the app's own directory, so a package shipping utility classes needs
// an `@source` line in every consumer — and forgetting it drops the classes silently, ONLY in a
// production build. Everything cosmetic is exposed as a data attribute for the consumer to style.

import { cloneElement, isValidElement, useId, type CSSProperties, type KeyboardEvent, type MouseEvent, type ReactElement, type ReactNode } from 'react';
import type { DegradedReason } from './types.js';

const REASON_TEXT: Readonly<Record<DegradedReason, string>> = {
  timeout: 'not answering (timed out)',
  network: 'not answering (unreachable)',
  unauthorized: 'not answering (your session was rejected)',
  'not-implemented': 'not answering (this build does not provide it)',
  'bad-response': 'not answering (unrecognised response)',
  'server-error': 'not answering (server error)',
  'cache-unavailable': 'not answering (discovery cache unavailable)',
};

export function reasonText(service: string, reason: DegradedReason): string {
  return `${service} is ${REASON_TEXT[reason]}`;
}

// Keyed on the data attribute, not a class or id: a class would need the `@source` consumer
// wiring this package deliberately avoids shipping (see the file header), and an id would
// collide across multiple <Capability> instances on one page. The `>` combinator is what reaches
// every child shape — a Fragment's children and a custom component's rendered root both land as
// direct children of this wrapper.
const DISABLED_CHILD_STYLE = "[data-capability-state='degraded'] > * { pointer-events: none; }";

export type CapabilityDisabledProps = {
  readonly service: string;
  readonly reason: DegradedReason;
  readonly children: ReactNode;
};

export function CapabilityDisabled({ service, reason, children }: CapabilityDisabledProps): ReactElement {
  // useId, never a hardcoded id: two <Capability> elements on one page would otherwise emit
  // duplicate ids and break the aria-describedby association for both.
  const descriptionId = useId();

  const block = (event: MouseEvent | KeyboardEvent): void => {
    event.preventDefault();
    event.stopPropagation();
  };

  // `aria-disabled` is not inherited, so a screen-reader user navigating by link — a very common
  // mode — lands directly on the child and hears only its name, with no disabled state and no
  // reason: the same failure by another route that `inert` was rejected for above. When `children`
  // is a single valid element (React already collapses one JSX child to a plain element, never an
  // array — `isValidElement` alone tells single from not, the same test @paigasus/ui's Field uses
  // for the identical "clone a caller-supplied child" problem), clone it and put both attributes on
  // it too, so the interactive node itself announces the state — and also clone on a belt-and-braces
  // `pointer-events: none` style (see the file header; the `<style>` rule below is what actually
  // carries the pre-hydration guarantee for every child shape). The wrapper keeps the aria
  // attributes regardless: a consumer may style off them, and `data-capability-state` lives there
  // either way. A non-single-element `children` (multiple children, a string, null) falls back to
  // the wrapper-only behaviour rather than throwing — it has no single child to clone the aria
  // attributes onto, but the `<style>` rule below still reaches each of its rendered direct
  // children, so it keeps the pre-hydration CSS guarantee even here.
  type ChildAriaProps = { 'aria-disabled'?: string; 'aria-describedby'?: string; style?: CSSProperties };
  const disabledChild = isValidElement<ChildAriaProps>(children)
    ? // Field (@paigasus/ui) carries the same justification: this component owns the association
      // between the wrapper's disabled state and a caller-supplied child it did not create, and
      // cloning is the only way to attach the aria attributes without asking every consumer to
      // thread them through by hand.
      // eslint-disable-next-line @eslint-react/no-clone-element -- see the comment above.
      cloneElement(children, {
        'aria-disabled': 'true',
        // Appended, not replaced: aria-describedby accepts a space-separated id list, and
        // dropping the child's own description would discard information the consumer
        // deliberately attached, not just ours.
        'aria-describedby': [children.props['aria-describedby'], descriptionId].filter(Boolean).join(' '),
        // Merged, not replaced: the child may already carry its own `style`, and clobbering it
        // would silently drop consumer styling rather than only adding this one property.
        // Belt-and-braces only — see the file header and the `DISABLED_CHILD_STYLE` rule below,
        // which is what actually makes the pre-hydration guarantee hold for every child shape.
        style: { ...children.props.style, pointerEvents: 'none' },
      })
    : null;

  return (
    <span
      data-capability-state="degraded"
      data-capability-reason={reason}
      aria-disabled="true"
      aria-describedby={descriptionId}
      onClickCapture={block}
      onKeyDownCapture={(event) => {
        if (event.key === 'Enter' || event.key === ' ') block(event);
      }}
    >
      <style>{DISABLED_CHILD_STYLE}</style>
      {disabledChild ?? children}
      <span
        id={descriptionId}
        style={{
          position: 'absolute',
          width: 1,
          height: 1,
          padding: 0,
          margin: -1,
          overflow: 'hidden',
          clip: 'rect(0 0 0 0)',
          whiteSpace: 'nowrap',
          border: 0,
        }}
      >
        {reasonText(service, reason)}
      </span>
    </span>
  );
}
