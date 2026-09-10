// SPDX-License-Identifier: Apache-2.0
import type { DiscoveryLogger } from '../ports/logger.js';

/** The default. The package emits nothing unless an app opts in. */
export const noopLogger: DiscoveryLogger = { event: () => undefined };
