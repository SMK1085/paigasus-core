// SPDX-License-Identifier: Apache-2.0
//
// SMA-512 PR 2, task 3 fix round 1. `iam-clients.ts`'s `withCorrelation` constructs
// `new Headers(options?.headers)`, where `options: CallOptions` is `@connectrpc/connect`'s own
// type and `CallOptions.headers?: HeadersInit`. `@types/node@24`'s `web-globals/fetch.d.ts`
// globally declares `Headers`, `Request`, `Response`, `FormData` etc. (sourced from `undici-types`)
// WITHOUT needing the "DOM" lib — this package's tsconfig.json comment is correct about that — but
// it does NOT declare `HeadersInit` itself, an apparent gap in @types/node's set. Since `connect`'s
// `call-options.d.ts` references `HeadersInit` as a bare global (never importing it), and nothing
// in this program declares that name, it silently resolves to the TypeScript "error" type — MEASURED
// with `type T = CallOptions['headers']; const x: number = null as unknown as T;`, which raises NO
// error, proving `T` is `any`/error, not a real type. `tsc` stays quiet (skipLibCheck suppresses the
// diagnostic on `call-options.d.ts` itself), but that `any` reaches `new Headers(options?.headers)`,
// which is exactly what `@typescript-eslint/no-unsafe-argument` in `ts:lint` correctly flags.
//
// The fix is this one missing global, not the "DOM" lib: adding "DOM" pulls in `window`/`document`
// and everything else, when only ONE name is missing. The shape below matches the WHATWG standard
// (a Headers instance, an array of [name, value] pairs, or a plain record) and — this is the part
// that must stay exact — matches `undici-types/fetch.d.ts`'s OWN internal `HeadersInit`
// (`Headers | [string, string][] | Record<string, string>`), which is what the `Headers`
// constructor itself actually expects: `Headers`'s constructor is resolved via that module
// directly (not through this global), so a shape mismatch here — `Iterable<[string, string]>`
// instead of `[string, string][]`, for instance — fails at the `new Headers(...)` call site with an
// assignability error, not a silent pass. MEASURED both ways.
//
// If a future `@types/node` bump adds its own global `HeadersInit`, this becomes a duplicate
// declaration (TS merges identical `type` aliases; a differently-shaped one errors loudly) — check
// this file first on that bump before assuming an unrelated regression.
declare global {
  type HeadersInit = Headers | [string, string][] | Record<string, string>;
}

export {};
