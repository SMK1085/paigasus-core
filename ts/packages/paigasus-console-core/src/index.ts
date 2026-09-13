// SPDX-License-Identifier: Apache-2.0
//
// The package's one entry. Server-only: every module it re-exports imports 'server-only', so a
// client component that reaches this package fails the build loudly rather than shipping a
// token-bearing module into a browser bundle.
import 'server-only';

export { principalPrnOf } from './principal-prn';
export { ROOT_PRN, isUuid, organizationPrn, parseTenancyPrn, projectPrn, teamPrn, type TenancyKind, type TenancyRef } from './prn-tenancy';
export { CORRELATION_HEADER, REQUEST_PATH_HEADER } from './correlation-header';
export { createJsonLogger, logger, type AppEventFields, type AppEventName, type ConsoleLogger } from './logger';
export { callIam, neverReachedIam, sessionExpired, type ActionState, type IamResult } from './errors';
export { FORBIDDEN_VIEW_CORRELATION, requestCorrelationId, requestPath } from './correlation';
export { createIamClients, type IamClients } from './iam-clients';
export { createIntrospectPrincipalResolver } from './principal-resolver';
export { setConsolePorts } from './runtime-ports';

// SMA-512 PR 2, task 4. The five cached accessors, still exporting their module-scope cache()
// wrappers as-is — task 5 converts them to a createConsoleRuntime() factory.
export { currentSession, iamClients, iamClientsForAction, optionalSession, sessionToken } from './iam';
export { currentPrincipal, introspectWithProvisioning, type Principal } from './principal';
export { createMayI, mayI, type IamAction, type MayI } from './authorize';
export { SCOPE_CAP, cedarCapabilityOf, loadMyScopes, myScopes, switcherOrgs, type MyScopes, type ScopeEntry } from './scopes';
export { createAppDiscovery, descriptorCacheFor, discovery, resetDiscoveryForTest } from './discovery';
