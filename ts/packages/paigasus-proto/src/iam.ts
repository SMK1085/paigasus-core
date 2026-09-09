// SPDX-License-Identifier: Apache-2.0
//
// The iam/v1 surface, as its OWN entry point rather than part of the root barrel.
//
// The reason is a name collision, not taste. iam.proto:22-33 keeps a DEPRECATED
// `ServiceInfo` message — dead, served by nothing, retained only because buf
// forbids deleting a message — and the root barrel already re-exports the LIVE
// `paigasus.common.v1.ServiceInfo`. Re-exporting both from one module is a
// duplicate-export error, and hand-enumerating every other iam/v1 name to dodge
// it would need editing on every proto change.
//
// This is a curated entry backed by this file, NOT a `./generated/*` passthrough.
// That distinction matters: a passthrough would make the generated file layout
// public API, so moving a generated directory would become a breaking change for
// consumers. Here the layout stays free to move behind this barrel.
export * from './generated/paigasus/iam/v1/iam_pb.js';
