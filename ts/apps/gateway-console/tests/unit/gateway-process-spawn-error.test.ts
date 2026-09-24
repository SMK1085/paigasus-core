// SPDX-License-Identifier: Apache-2.0
//
// A spawn failure (ENOENT, EACCES, ...) emits 'error' on the ChildProcess, after existsSync(
// GATEWAY_BIN) already passed. With no listener that event is unhandled and crashes the worker.
// This test drives the real 'error' event through gateway-process.ts's spawn() call and asserts
// startGateway() turns it into a rejected promise instead — never an uncaught exception. It mocks
// 'node:child_process', 'node:fs' and './harness' rather than importing them for real: harness.ts
// calls @playwright/test's `test.extend` at module scope, which must not run under vitest
// (see gateway-env.test.ts's own comment on the same constraint).
import { EventEmitter } from 'node:events';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { existsSyncMock, spawnMock, freePortMock, stopMock, waitForHealthMock } = vi.hoisted(() => ({
  existsSyncMock: vi.fn(),
  spawnMock: vi.fn(),
  freePortMock: vi.fn(),
  stopMock: vi.fn(),
  waitForHealthMock: vi.fn(),
}));

vi.mock('node:fs', () => ({ existsSync: existsSyncMock }));
vi.mock('node:child_process', () => ({ spawn: spawnMock }));
vi.mock('../e2e/support/harness', () => ({
  freePort: freePortMock,
  stop: stopMock,
  waitForHealth: waitForHealthMock,
}));

// Imported after the mocks above so gateway-process.ts's own `import ... from './harness'`
// resolves to the mock, not the real @playwright/test-importing module.
const { GATEWAY_BIN, startGateway } = await import('../e2e/support/gateway-process');

/** A minimal stand-in for the ChildProcess spawn() returns: an EventEmitter plus the fields and
 * method gateway-process.ts and harness.ts's stop() read. stop() itself is mocked away here, so
 * kill() is never exercised. */
function fakeChild(): EventEmitter & { exitCode: number | null; signalCode: string | null; stdout: EventEmitter; stderr: EventEmitter; kill: () => boolean } {
  return Object.assign(new EventEmitter(), {
    exitCode: null,
    signalCode: null,
    stdout: new EventEmitter(),
    stderr: new EventEmitter(),
    kill: vi.fn(() => true),
  });
}

describe('startGateway spawn failure', () => {
  beforeEach(() => {
    existsSyncMock.mockImplementation((p: string) => !p.endsWith('gateway.toml'));
    freePortMock.mockResolvedValue(4321);
    stopMock.mockResolvedValue(undefined);
    // Never resolves: the test always wins the race through the 'error' event, exactly as a real
    // spawn failure does (the health probe never gets a listening port to hit).
    waitForHealthMock.mockReturnValue(new Promise(() => {}));
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it('turns an unhandled child "error" event into a rejected promise naming the binary and the cause', async () => {
    const child = fakeChild();
    spawnMock.mockReturnValue(child);

    const result = startGateway({ iamGrpcUrl: 'http://iam.test', openAiUrl: 'http://openai.test' });
    await vi.waitFor(() => expect(spawnMock).toHaveBeenCalled());

    // exitCode is set before Node fires 'error' on a real spawn failure (measured against
    // Node's own child_process implementation); this fake mirrors that ordering.
    child.exitCode = -2;
    const cause = Object.assign(new Error('spawn /rs/target/debug/paigasus-gateway ENOENT'), { code: 'ENOENT' });
    child.emit('error', cause);

    await expect(result).rejects.toThrow(new RegExp(`failed to spawn ${GATEWAY_BIN.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`));
    await expect(result).rejects.toMatchObject({ cause });
    // The listener this fix adds must be the only thing standing between the 'error' event and
    // Node's default behaviour (rethrowing on the next tick and crashing the process). Getting a
    // rejection at all, rather than an uncaught exception, is the proof; this also confirms the
    // module cleaned the child up rather than leaving it dangling.
    expect(stopMock).toHaveBeenCalledWith(child);
  });
});
