// SPDX-License-Identifier: Apache-2.0
//
// The vocabulary /client and /server both speak. This module holds NO server machinery — no
// store, no OIDC client, no `server-only` import, nothing that could not run in a browser bundle.
// That is what lets src/client.ts (React-only, AC 5) import it directly while every path under
// src/core/**, src/adapters/**, and src/ports/** stays banned at the client boundary (see
// eslint.mjs's paigasus/boundaries/auth-client block). It started out living inside
// src/core/session.ts, which reads as server territory even for a type-only reach across it — the
// eslint rule bans `./core/**` on purpose, including type imports, so the fix was to move the
// shared surface out of `core/` rather than carve an exception into the rule.
//
// RoleGrantRef is defined HERE, not in ports/principal-resolver.ts, because SessionView needs it
// and client.ts must not reach ./ports/** either. principal-resolver.ts imports it from here
// instead of declaring a second copy, so the two never drift apart.

/** A role grant, flattened from IAM's RoleGrantRef { scope_prn, role_key }. */
export interface RoleGrantRef {
  scopePrn: string;
  roleKey: string;
}

/**
 * Everything /client may ever see. A runtime tuple, not just a type: an interface has no runtime
 * key set, so a strict-equality assertion needs something to compare against.
 */
export const SESSION_VIEW_KEYS = ['principalPrn', 'displayName', 'email', 'grants', 'grantsAvailable'] as const;

export type SessionViewKey = (typeof SESSION_VIEW_KEYS)[number];

export interface SessionView {
  principalPrn: string | null;
  displayName: string | null;
  email: string | null;
  grants: RoleGrantRef[];
  grantsAvailable: boolean;
}
