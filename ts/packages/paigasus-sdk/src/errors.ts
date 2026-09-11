// SPDX-License-Identifier: Apache-2.0
//
// The `./errors` entry. It is GUARDED and lives at src/ root. tests/server-guard.test.ts derives
// the guard import each entry must open with from the entry's own depth ("import
// './server-guard';" here).
//
// mapError runs in the BFF. A client component receives an already-mapped PaigasusError as a
// serializable prop and imports its TYPES from './errors/types' instead (spec § 6.3).
import './server-guard';

export { mapError } from './errors/map-error';
export type { ErrorInput } from './errors/map-error';
export { presentationOverride } from './errors/presentation';
export { grpcCodeName, presentationForGrpcCode, presentationForHttpStatus, presentationForTransportCause } from './errors/transport-status';
export { ErrorDomain, ErrorReason } from './errors/types';
export type { PaigasusError, Presentation, TransportCause, TransportInfo } from './errors/types';
