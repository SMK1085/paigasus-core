// SPDX-License-Identifier: Apache-2.0
import type { AuditMetadata } from './generated/paigasus/common/v1/audit_pb.js';

/**
 * Structural interface satisfied by any generated message embedding AuditMetadata.
 *
 * `audit?: AuditMetadata | undefined` mirrors protobuf-es's own idiom for optional
 * message fields. The explicit `| undefined` is required (not redundant) under
 * `exactOptionalPropertyTypes`: protobuf-es types the field as `AuditMetadata | undefined`,
 * and a bare `audit?: AuditMetadata` would forbid `undefined`-when-present, making the
 * generated messages fail to structurally satisfy this interface.
 */
export interface Auditable {
  audit?: AuditMetadata | undefined;
}
