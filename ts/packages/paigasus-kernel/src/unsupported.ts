// SPDX-License-Identifier: Apache-2.0
const UNSUPPORTED_ERROR = '@paigasus/kernel has no browser/Edge binding yet — wasm (paigasus-wasm) is a tracked follow-up';

// Resolved by the `default` (browser/Edge/workerd) export condition. A callable `sum` (not a
// module-load throw) so an `import { sum }` named import RESOLVES, then throws on call with a clear
// message — instead of breaking named-export resolution (CodeRabbit). Typecheck always resolves the
// `node` condition (tsconfig customConditions), so the param-less stub is invisible to consumers.
export function sum(): never {
  throw new Error(UNSUPPORTED_ERROR);
}
