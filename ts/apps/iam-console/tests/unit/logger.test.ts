// SPDX-License-Identifier: Apache-2.0
//
// lib/logger.ts (spec § 4.8, § 9.2): it writes the event name, a time and EXACTLY the fields the
// port gave it — nothing it could have picked up elsewhere, so never a DSN or a token.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createJsonLogger } from '../../lib/logger';

afterEach(() => {
  vi.unstubAllEnvs();
});

function capture() {
  const lines: string[] = [];
  const logger = createJsonLogger((line) => lines.push(line));
  const parsed = () => lines.map((line) => JSON.parse(line) as { time: string; event: string; fields: Record<string, unknown> });
  return { logger, lines, parsed };
}

describe('createJsonLogger', () => {
  it('writes one JSON line per auth event, with the fields under their own key', () => {
    const { logger, lines, parsed } = capture();
    logger.event('session.created', { sid: 'abcd1234', zone: 'iam' });
    expect(lines).toHaveLength(1);
    const [line] = parsed();
    expect(Object.keys(line ?? {}).sort()).toEqual(['event', 'fields', 'time']);
    expect(line?.event).toBe('session.created');
    expect(line?.fields).toEqual({ sid: 'abcd1234', zone: 'iam' });
    expect(Number.isNaN(Date.parse(line?.time ?? ''))).toBe(false);
  });

  it('writes discovery events and app events the same way', () => {
    const { logger, parsed } = capture();
    logger.event('discovery.probe_failed', { service: 'iam', reason: 'timeout' });
    logger.appEvent('principal.resolve_failed', { presentation: 'degraded', correlation_id: null });
    expect(parsed().map((line) => [line.event, line.fields])).toEqual([
      ['discovery.probe_failed', { service: 'iam', reason: 'timeout' }],
      ['principal.resolve_failed', { presentation: 'degraded', correlation_id: null }],
    ]);
  });

  // The DSN is really in the environment, in the variable the Redis connect path reads. So a logger
  // that appends the env (or any key beyond time/event/fields) fails both assertions.
  it('writes exactly the given fields, and never the Redis DSN that is in its environment', () => {
    const dsn = 'redis://console:s3cret-password@redis.internal:6379/0';
    vi.stubEnv('PAIGASUS_SESSION_REDIS_URL', dsn);
    const { logger, lines } = capture();
    logger.appEvent('discovery.redis_connect_failed', { stage: 'connect' });
    expect(lines).toHaveLength(1);
    // The FULL record, by toEqual: any key the logger adds beyond these three fails it. The time
    // is taken from the record itself, and checked on its own below.
    const record = JSON.parse(lines[0] ?? '{}') as Record<string, unknown>;
    expect(record).toEqual({ time: record['time'], event: 'discovery.redis_connect_failed', fields: { stage: 'connect' } });
    expect(typeof record['time']).toBe('string');
    const output = lines.join('\n');
    expect(output).not.toContain(dsn);
    expect(output).not.toContain('s3cret-password');
  });
});
