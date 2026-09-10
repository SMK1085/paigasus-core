// SPDX-License-Identifier: Apache-2.0
//
// The `./errors` entry. It is GUARDED and lives at src/ root: tests/server-guard.test.ts:19 pins
// the literal "import './server-guard.js';" and compares it with toBe at :52, so an entry one
// directory deep would need '../server-guard.js' and fail.
//
// mapError runs in the BFF. A client component receives an already-mapped PaigasusError as a
// serializable prop and imports its TYPES from './errors/types' instead (spec § 6.3).
import './server-guard.js';

export { mapError } from './errors/map-error.js';
export type { ErrorInput } from './errors/map-error.js';
export { presentationOverride } from './errors/presentation.js';
export { grpcCodeName, presentationForGrpcCode, presentationForHttpStatus, presentationForTransportCause } from './errors/transport-status.js';
export { ErrorDomain, ErrorReason } from './errors/types.js';
export type { PaigasusError, Presentation, TransportCause, TransportInfo } from './errors/types.js';
