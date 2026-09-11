// SPDX-License-Identifier: Apache-2.0
//
// The SECOND client-safe entry (./types is the first), for @paigasus/app-shell (SMA-510 spec § 9.2).
//
// NO directive and NO `server-only`. It re-exports from two kinds of module:
// - reasonText and CapabilityDisabled come from ./disabled, which keeps its own client directive.
//   A client module calls reasonText to render a degraded nav entry.
// - capabilityOutcome comes from ./core/outcome, a pure function with no directive. A SERVER module
//   can call it through this entry: app-shell's navStateOf runs in a server layout.
//
// EXTENSIONLESS relative specifiers: a consuming Next app compiles this graph, and Turbopack
// (Next 16.3.4) does not map './x.js' to './x.ts' (measured, SMA-510).
// tests/structure/exports.test.ts pins this file's import set and that rule.
export { CapabilityDisabled, reasonText, type CapabilityDisabledProps } from './disabled';
export { capabilityOutcome, type CapabilityOutcome } from './core/outcome';
