// SPDX-License-Identifier: Apache-2.0
//
// The root barrel. It re-exports the ./iam surface so `@paigasus/sdk` and `@paigasus/sdk/iam` are
// interchangeable for a consumer that wants everything; the subpath exists so a caller that only
// needs one surface does not pull the rest into its module graph (spec § 6.1).
import './server-guard.js';

export * from './iam.js';
