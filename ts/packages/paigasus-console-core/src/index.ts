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
export { NAME_MAX_CODE_POINTS, formFields, invalidFormInput, nameField, prnField, toActionResult, type ActionResult, type FormAction } from './form';
export { FORBIDDEN_VIEW_CORRELATION, requestCorrelationId, requestPath } from './correlation';
export { createIamClients, type IamClients } from './iam-clients';
export { createIntrospectPrincipalResolver } from './principal-resolver';
export { createConsoleRuntime, type ConsoleRuntime } from './runtime';

export { introspectWithProvisioning, type Principal } from './principal';
export { IAM_ACTIONS, createMayI, type IamAction, type MayI } from './authorize';
export { SCOPE_CAP, cedarCapabilityOf, loadMyScopes, switcherOrgs, type MyScopes, type ScopeEntry } from './scopes';
export { createAppDiscovery, descriptorCacheFor, resetDiscoveryForTest } from './discovery';
