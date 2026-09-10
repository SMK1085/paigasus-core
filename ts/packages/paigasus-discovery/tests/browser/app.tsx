// SPDX-License-Identifier: Apache-2.0
//
// Shared between fixture-server.ts (which renders one of these variants to real HTML
// SERVER-SIDE, giving the JS-disabled specs genuine markup to click) and entry.tsx (which
// HYDRATES the exact same tree client-side, picking the same variant fixture-server.ts chose —
// see entry.tsx's own comment). One set of components for both keeps "what got rendered" and
// "what React attaches to" from drifting into hand-written copies of the same tree.
//
// SMA-509 finding A added the `fragment` and `custom-component` variants: the pre-hydration
// guarantee `<Capability>`'s `<style>` rule (src/disabled.tsx) exists to give does not depend on
// `children` being a single plain element whose `style` prop reaches its own DOM node — a
// Fragment ignores `style` entirely, and a custom component may not forward it. Every variant's
// child carries its OWN `href`, distinct from anything the ancestor does — the pre-hydration
// spec's only reliable, script-free signal is whether clicking navigates to THIS href — a
// script-driven "ancestor" flag cannot fire at all before hydration, regardless of the CSS under
// test. See capability-hit-test.spec.ts's header for the full reasoning.
import type { CSSProperties, ReactElement, ReactNode } from 'react';
import { CapabilityDisabled } from '../../src/disabled.js';

export const CHILD_HREF = '/audit-target';

export function markAncestorClicked(): void {
  (window as Window & { __ancestorClicked?: boolean }).__ancestorClicked = true;
}

// A positive hydration control (SMA-509 finding C): always enabled, so a click on it proves
// `hydrateRoot` actually ran before the real, disabled-child assertion is trusted. Only the
// `default` variant renders it — it is only the post-hydration spec that needs it.
export function markControlClicked(): void {
  (window as Window & { __controlClicked?: boolean }).__controlClicked = true;
}

function Ancestor({ children }: { readonly children: ReactNode }): ReactElement {
  return (
    <div
      data-testid="ancestor"
      role="button"
      tabIndex={0}
      onClick={markAncestorClicked}
      onKeyDown={(event) => {
        if (event.key === 'Enter' || event.key === ' ') markAncestorClicked();
      }}
    >
      <p>A large clickable row — the composition the design change exists for.</p>
      {children}
    </div>
  );
}

type NonForwardingLinkProps = {
  readonly href: string;
  readonly children: ReactNode;
  // CapabilityDisabled's cloneElement adds these three props to whatever `children` is. This
  // component destructures only `href`/`children` and drops the rest — deliberately, to model a
  // custom component that never forwards `style` to its own rendered DOM node (SMA-509 finding A).
  readonly style?: CSSProperties;
  readonly 'aria-disabled'?: string;
  readonly 'aria-describedby'?: string;
};

function NonForwardingLink({ href, children }: NonForwardingLinkProps): ReactElement {
  return (
    <a href={href} data-testid="child-link">
      {children}
    </a>
  );
}

function DefaultApp(): ReactElement {
  return (
    <>
      <button type="button" data-testid="hydration-control" onClick={markControlClicked}>
        Control
      </button>
      <Ancestor>
        <CapabilityDisabled service="iam" reason="network">
          <a href={CHILD_HREF} data-testid="child-link">
            Audit
          </a>
        </CapabilityDisabled>
      </Ancestor>
    </>
  );
}

function FragmentChildApp(): ReactElement {
  return (
    <Ancestor>
      <CapabilityDisabled service="iam" reason="network">
        <>
          <a href={CHILD_HREF} data-testid="child-link">
            Audit
          </a>
        </>
      </CapabilityDisabled>
    </Ancestor>
  );
}

function CustomComponentChildApp(): ReactElement {
  return (
    <Ancestor>
      <CapabilityDisabled service="iam" reason="network">
        <NonForwardingLink href={CHILD_HREF}>Audit</NonForwardingLink>
      </CapabilityDisabled>
    </Ancestor>
  );
}

export const APPS = {
  default: DefaultApp,
  fragment: FragmentChildApp,
  'custom-component': CustomComponentChildApp,
} as const;

export type AppVariant = keyof typeof APPS;
