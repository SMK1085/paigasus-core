// SPDX-License-Identifier: Apache-2.0
//
// The root barrel. Task 4 turns this into a re-export of the ./iam surface, in the same commit that
// creates src/iam.ts and adds the "./iam" entry to package.json — the three belong together, so
// that no commit leaves the exports map pointing at a file that does not exist.
import './server-guard.js';

export {};
