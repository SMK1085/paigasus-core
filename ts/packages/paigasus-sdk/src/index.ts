// SPDX-License-Identifier: Apache-2.0
//
// The root barrel. It re-exports every subpath so `@paigasus/sdk` serves a consumer that wants
// everything; the subpaths exist so a caller needing one surface does not pull the rest into its
// module graph (spec § 6.1).
//
// Each name is listed EXPLICITLY. `export *` from two modules that share a name drops the
// ambiguous name SILENTLY under the ES semantics TypeScript follows — no error, no warning — the
// exact failure ts/packages/paigasus-proto/src/iam.ts:5-14 documents for `ServiceInfo`.
import './server-guard.js';

export { AuditService, AuthnService, AuthorizationService, OutboxService, ServiceAccountService, TenancyService, UserService, bindAuth, createIamClient, disposeTransports } from './iam.js';
export type { Auth, TransportOptions } from './iam.js';

export { DEFAULT_HEADER_TIMEOUT_MS, createChatClient, createTerminalFrameParser } from './chat.js';
export type { ChatClient, ChatClientOptions, ChatResult } from './chat.js';

export { ErrorDomain, ErrorReason, PRESENTATION, grpcCodeName, mapError, presentationForGrpcCode, presentationForHttpStatus, presentationForTransportCause, presentationOverride } from './errors.js';
export type { ErrorInput, PaigasusError, Presentation, PresentationEntry, TransportCause, TransportInfo } from './errors.js';
