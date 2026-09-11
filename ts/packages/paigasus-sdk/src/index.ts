// SPDX-License-Identifier: Apache-2.0
//
// The root barrel. It re-exports every subpath so `@paigasus/sdk` serves a consumer that wants
// everything; the subpaths exist so a caller needing one surface does not pull the rest into its
// module graph (spec § 6.1).
//
// Each name is listed EXPLICITLY. `export *` from two modules that share a name drops the
// ambiguous name SILENTLY under the ES semantics TypeScript follows — no error, no warning — the
// exact failure ts/packages/paigasus-proto/src/iam.ts:5-14 documents for `ServiceInfo`.
import './server-guard';

export {
  AuditService,
  AuthnService,
  AuthorizationService,
  OutboxService,
  ServiceAccountService,
  ServiceInfoService,
  TenancyService,
  UserService,
  bindAuth,
  createIamClient,
  disposeTransports,
} from './iam';
export type { Auth, TransportOptions } from './iam';

export { DEFAULT_HEADER_TIMEOUT_MS, createChatClient, createTerminalFrameParser } from './chat';
export type { ChatClient, ChatClientOptions, ChatResult } from './chat';

export { ErrorDomain, ErrorReason, grpcCodeName, mapError, presentationForGrpcCode, presentationForHttpStatus, presentationForTransportCause, presentationOverride } from './errors';
export type { ErrorInput, PaigasusError, Presentation, TransportCause, TransportInfo } from './errors';
