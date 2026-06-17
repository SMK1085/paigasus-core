/**
 * Browser-callable wrapper over [`paigasus_kernel::sum`]. Uses `i32` at the FFI boundary so the
 * JS surface is a plain `number` (matching the napi binding); the kernel fn is `i64`, cast at the
 * boundary. A future kernel fn needing the full `i64` range gets explicit handling then (shared
 * across all bindings — SMA-427 L5).
 * @param {number} a
 * @param {number} b
 * @returns {number}
 */
export function sum(a, b) {
    const ret = wasm.sum(a, b);
    return ret;
}
export function __wbindgen_init_externref_table() {
    const table = wasm.__wbindgen_externrefs;
    const offset = table.grow(4);
    table.set(0, undefined);
    table.set(offset + 0, undefined);
    table.set(offset + 1, null);
    table.set(offset + 2, true);
    table.set(offset + 3, false);
}

let wasm;
export function __wbg_set_wasm(val) {
    wasm = val;
}
