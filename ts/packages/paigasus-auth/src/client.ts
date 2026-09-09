// SPDX-License-Identifier: Apache-2.0
//
// The ONLY package entry a browser bundle may reach (AC 5). It imports NOTHING beyond `react` and
// the `SessionView` type — no store, no OIDC client, no `server-only` module, nothing that could
// ever hold a token. A client bundle built from this file therefore cannot embed a credential even
// by accident, and there is no import graph left to audit here the way
// `tests/middleware.test.ts` audits `src/middleware.ts`'s — this file's own import list already
// says everything.
//
// Because JSX syntax needs a `.tsx` file extension and this file is `.ts` (a package.json
// `exports` fact, not a style choice), `SessionProvider` is built with `createElement` rather than
// JSX.
'use client';

import { createContext, createElement, useContext, type ReactElement, type ReactNode } from 'react';
import type { SessionView } from './core/session.js';

export type { SessionView };

const SessionContext = createContext<SessionView | undefined>(undefined);

export interface SessionProviderProps {
  /** The view for the current request, produced server-side by `toSessionView` (`./server`). */
  value: SessionView;
  children: ReactNode;
}

/** Hands the server-resolved `SessionView` down to `useSession()`. Never fetches; holds no token. */
export function SessionProvider(props: SessionProviderProps): ReactElement {
  return createElement(SessionContext, { value: props.value }, props.children);
}

/**
 * Thrown by `useSession()` when called outside a `SessionProvider` — a distinct, named failure
 * instead of silently returning `undefined` to a caller that assumed a session was always present.
 */
export class UseSessionOutsideProviderError extends Error {
  constructor() {
    super('useSession() was called outside a SessionProvider');
    this.name = 'UseSessionOutsideProviderError';
  }
}

export function useSession(): SessionView {
  const value = useContext(SessionContext);
  if (value === undefined) {
    throw new UseSessionOutsideProviderError();
  }
  return value;
}

/**
 * FAILS OPEN when `grantsAvailable` is false. This is deliberate and must not be "simplified" away
 * by a future reader.
 *
 * `SessionView.grantsAvailable` is false while the claims-based resolver is in use
 * (adapters/claims-resolver.ts) — SMA-508 wires the real one. An unavailable grant set means
 * UNKNOWN, never DENIED (see `ResolvedPrincipal.grantsAvailable` in
 * ports/principal-resolver.ts). If `can()` returned `false` for every check while grants are
 * unavailable, `@paigasus/app-shell` (SMA-510) would gate its navigation on it and render a
 * console with NO navigation at all — and IAM, the actual authorizer, would never even be asked.
 * Failing open here is correct precisely because this check is cosmetic (navigation visibility),
 * never the authorization decision itself: every mutating action is re-checked authoritatively by
 * IAM server-side, regardless of what this function returns.
 */
export function can(session: SessionView, required: { scopePrn: string; roleKey: string }): boolean {
  if (!session.grantsAvailable) return true;
  return session.grants.some((grant) => grant.scopePrn === required.scopePrn && grant.roleKey === required.roleKey);
}
