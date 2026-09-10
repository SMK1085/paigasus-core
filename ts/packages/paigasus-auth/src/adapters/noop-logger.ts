// SPDX-License-Identifier: Apache-2.0
import type { AuthLogger } from '../ports/logger.js';

/** The default. The package emits nothing unless an app opts in. */
export const noopLogger: AuthLogger = { event: () => undefined };
