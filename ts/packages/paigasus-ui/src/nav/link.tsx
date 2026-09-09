// SPDX-License-Identifier: Apache-2.0
'use client';

import { createContext, use, type ComponentType, type ReactElement, type ReactNode } from 'react';

/**
 * The navigation contract (ADR-0021 decision 3, SMA-503 AC 1).
 *
 * @paigasus/ui imports nothing from next/*. The package defines this contract; the
 * application supplies the implementation. SMA-510's app shell consumes the same contract.
 */
export interface LinkProps {
  href: string;
  className?: string;
  children?: ReactNode;
}

export type LinkComponent = ComponentType<LinkProps>;

function AnchorLink({ href, className, children }: LinkProps): ReactElement {
  return (
    <a href={href} className={className}>
      {children}
    </a>
  );
}

/*
 * The context default is a plain <a>. That default is what makes AC 2 work: a component tree
 * renders in jsdom with no provider and no Next runtime present.
 */
const LinkContext = createContext<LinkComponent>(AnchorLink);

export function LinkProvider({ link, children }: { link: LinkComponent; children: ReactNode }): ReactElement {
  // React 19 renders a context object directly as its own provider; <LinkContext.Provider> is
  // the legacy spelling (@eslint-react/no-context-provider). This package targets React 19.
  return <LinkContext value={link}>{children}</LinkContext>;
}

export function useLinkComponent(): LinkComponent {
  // `use` supersedes `useContext` in React 19 (@eslint-react/no-use-context).
  return use(LinkContext);
}

/*
 * `Resolved` below is a component READ OUT OF CONTEXT, not one defined during this render. Its
 * identity is owned by whoever called `LinkProvider` — or, with no provider, by the
 * module-level `AnchorLink` — so it does not change between renders, and the state-resetting
 * remount that `static-components` exists to catch cannot happen here. Two rules
 * (`@eslint-react/static-components` and `react-hooks/static-components`) report the same
 * pattern, at both the assignment and the JSX tag, so all four sites are disabled explicitly.
 *
 * This is a SCOPED disable carrying its rebuttal, matching form.tsx's handling of
 * `@eslint-react/no-clone-element` (SMA-503 fix round 2, item 10). The previous spelling,
 * `createElement(Resolved, props)`, dodged the rules instead of answering them and left two
 * philosophies in one package.
 */
export function Link(props: LinkProps): ReactElement {
  // eslint-disable-next-line @eslint-react/static-components -- see the comment above: Resolved comes from context and is stable across renders.
  const Resolved = useLinkComponent();
  // eslint-disable-next-line @eslint-react/static-components, react-hooks/static-components -- see the comment above: Resolved comes from context and is stable across renders.
  return <Resolved {...props} />;
}
