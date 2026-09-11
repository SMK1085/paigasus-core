// SPDX-License-Identifier: Apache-2.0
//
// The iam/v1 surface, as its OWN entry point rather than part of the root barrel.
//
// The reason is a name collision, not taste. iam.proto:22-33 keeps a DEPRECATED
// `ServiceInfo` message — dead, served by nothing, retained only because buf
// forbids deleting a message — and the root barrel already re-exports the LIVE
// `paigasus.common.v1.ServiceInfo`. A star re-export does NOT error: TypeScript's
// ES export semantics let the root barrel's explicit `ServiceInfoSchema` export
// SHADOW the deprecated iam/v1 one silently, so the root barrel would quietly
// serve the wrong `ServiceInfo` to anyone reaching for the iam one — a worse
// failure than a compile error, since nothing tells you it happened. Hand-
// enumerating every other iam/v1 name to dodge the shadowing would need editing
// on every proto change.
//
// This is a curated entry backed by this file, NOT a `./generated/*` passthrough.
// That distinction matters: a passthrough would make the generated file layout
// public API, so moving a generated directory would become a breaking change for
// consumers. Here the layout stays free to move behind this barrel.
export * from './generated/paigasus/iam/v1/iam_pb';
