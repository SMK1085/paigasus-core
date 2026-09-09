// SPDX-License-Identifier: Apache-2.0
import { MemorySessionStore } from '../../src/adapters/memory-store.js';
import { runStoreContract } from '../store-contract.js';

runStoreContract('memory', () => Promise.resolve(new MemorySessionStore()));
