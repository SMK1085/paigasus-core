// SPDX-License-Identifier: Apache-2.0
import { createMemoryDescriptorCache } from '../src/adapters/memory-cache.js';
import { runCacheContract } from './cache-contract.js';

runCacheContract('memory', async () => createMemoryDescriptorCache());
