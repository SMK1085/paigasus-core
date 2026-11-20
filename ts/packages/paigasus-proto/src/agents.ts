// SPDX-License-Identifier: Apache-2.0
//
// The agents/v1 surface, as its OWN entry point. The root barrel does not export it
// (same as `src/iam.ts`). This is a curated entry, not a `./generated/*` passthrough,
// so the generated layout stays free to move behind this file.
export * from './generated/paigasus/agents/v1/agents_pb';
