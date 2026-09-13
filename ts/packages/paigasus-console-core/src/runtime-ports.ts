// SPDX-License-Identifier: Apache-2.0
import 'server-only';
import type { AuthRuntime } from '@paigasus/auth/server';
import type { ConsoleCoreConfig } from './config-shape';

type Ports = { authRuntime: () => Promise<AuthRuntime>; config: () => ConsoleCoreConfig };
let ports: Ports | null = null;

/** Set once, at module scope, by the app's createConsoleRuntime() call (task 5). */
export function setConsolePorts(next: Ports): void {
  ports = next;
}

export function consolePorts(): Ports {
  if (ports === null) {
    throw new Error('@paigasus/console-core: createConsoleRuntime() was never called. An app must call it once, at module scope, before any accessor runs.');
  }
  return ports;
}
