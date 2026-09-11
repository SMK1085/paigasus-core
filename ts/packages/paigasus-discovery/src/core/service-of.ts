// SPDX-License-Identifier: Apache-2.0
//
// An import-free LEAF, on purpose (SMA-510). core/outcome.ts needs serviceOf, and the client-safe
// ./client entry reaches core/outcome.ts. core/state.ts imports @paigasus/proto as a VALUE, so
// reaching serviceOf through state.ts would put protobuf-es into every client bundle that renders
// a nav. state.ts re-exports this function, so its existing importers do not change.

/** The service slug a capability key belongs to. */
export function serviceOf(key: string): string {
  const dot = key.indexOf('.');
  return dot === -1 ? key : key.slice(0, dot);
}
