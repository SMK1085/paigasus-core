// SPDX-License-Identifier: Apache-2.0
export {}; // module marker (isolatedModules); no runtime exports.
// Compile-time guard (SMA-427 M5 / SMA-448): the wasm and napi FFI surfaces must stay
// type-identical, because `@paigasus/kernel`'s typecheck only resolves the `node` (napi) condition
// (tsconfig customConditions), so the shipped browser surface is otherwise never type-checked. No
// runtime effect; `tsc --noEmit` fails the build if any binding signature drifts.
//
// `typeof import(...)` is a pure type query (no import statement), safe under verbatimModuleSyntax +
// isolatedModules.
type NapiApi = typeof import('@paigasus/node-bindings');
type WasmApi = typeof import('@paigasus/wasm');

// `Exact<A, B>` is `true` only when A and B are mutually assignable; otherwise `never`, so the
// `: Exact<...> = true` binding fails to compile.
type Exact<A, B> = A extends B ? (B extends A ? true : never) : never;

const _sum: Exact<NapiApi['sum'], WasmApi['sum']> = true;
const _prnCanonicalize: Exact<NapiApi['prnCanonicalize'], WasmApi['prnCanonicalize']> = true;
const _prnErrorKind: Exact<NapiApi['prnErrorKind'], WasmApi['prnErrorKind']> = true;
const _prnBuild: Exact<NapiApi['prnBuild'], WasmApi['prnBuild']> = true;
const _prnService: Exact<NapiApi['prnService'], WasmApi['prnService']> = true;
const _prnRegion: Exact<NapiApi['prnRegion'], WasmApi['prnRegion']> = true;
const _prnOrg: Exact<NapiApi['prnOrg'], WasmApi['prnOrg']> = true;
const _prnResourceType: Exact<NapiApi['prnResourceType'], WasmApi['prnResourceType']> = true;
const _prnResourceId: Exact<NapiApi['prnResourceId'], WasmApi['prnResourceId']> = true;
const _mintUuid7: Exact<NapiApi['mintUuid7'], WasmApi['mintUuid7']> = true;
const _prnCedarEntityType: Exact<NapiApi['prnCedarEntityType'], WasmApi['prnCedarEntityType']> = true;
const _prnCedarEntityId: Exact<NapiApi['prnCedarEntityId'], WasmApi['prnCedarEntityId']> = true;

void _sum;
void _prnCanonicalize;
void _prnErrorKind;
void _prnBuild;
void _prnService;
void _prnRegion;
void _prnOrg;
void _prnResourceType;
void _prnResourceId;
void _mintUuid7;
void _prnCedarEntityType;
void _prnCedarEntityId;
