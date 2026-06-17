// SPDX-License-Identifier: Apache-2.0
export {}; // module marker (isolatedModules); no runtime exports.
// Compile-time guard (SMA-427 M5): the wasm and napi `sum` surfaces must stay type-identical, because
// `@paigasus/kernel`'s typecheck only ever resolves the `node` (napi) condition (tsconfig
// customConditions), so the shipped browser surface is otherwise never type-checked. No runtime effect;
// `tsc --noEmit` fails the build if either binding's `sum` signature drifts.
//
// Uses `typeof import(...)` (a pure type query, no import statement) so it is safe under the repo's
// `verbatimModuleSyntax` + `isolatedModules` — an `import type { sum }` + `typeof sum` would be illegal
// (can't use a type-only binding as a value).
type NapiApi = typeof import('@paigasus/node-bindings');
type WasmApi = typeof import('@paigasus/wasm');

// If a signature diverges, the corresponding alias becomes `never` and the `= true` lines fail to compile.
type _NapiSumAssignableToWasm = NapiApi['sum'] extends WasmApi['sum'] ? true : never;
type _WasmSumAssignableToNapi = WasmApi['sum'] extends NapiApi['sum'] ? true : never;

const _napiOk: _NapiSumAssignableToWasm = true;
const _wasmOk: _WasmSumAssignableToNapi = true;
void _napiOk;
void _wasmOk;
