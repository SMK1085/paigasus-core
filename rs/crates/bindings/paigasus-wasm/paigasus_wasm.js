/* @ts-self-types="./paigasus_wasm.d.ts" */
import * as wasm from "./paigasus_wasm_bg.wasm";
import { __wbg_set_wasm } from "./paigasus_wasm_bg.js";

__wbg_set_wasm(wasm);
wasm.__wbindgen_start();
export {
    mintUuid7, prnBuild, prnCanonicalize, prnCedarEntityId, prnCedarEntityType, prnErrorKind, prnOrg, prnRegion, prnResourceId, prnResourceType, prnService, sum
} from "./paigasus_wasm_bg.js";
