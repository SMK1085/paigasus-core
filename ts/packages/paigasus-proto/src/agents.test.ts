// SPDX-License-Identifier: Apache-2.0
import { create, fromJson } from '@bufbuild/protobuf';
import { describe, expect, it } from 'vitest';
import { OutboxMessageSchema, TimerFiredSchema, TimerKind, TimerOrigin } from './agents';

describe('agents/v1 types', () => {
  it('creates an OutboxMessage', () => {
    const msg = create(OutboxMessageSchema, { runPrn: 'p', text: 'hi' });
    expect(msg.runPrn).toBe('p');
    expect(msg.text).toBe('hi');
  });

  it('has the agents type name', () => {
    expect(OutboxMessageSchema.typeName).toBe('paigasus.agents.v1.OutboxMessage');
  });

  it('parses the follow-up TimerFired golden', () => {
    const msg = fromJson(TimerFiredSchema, {
      timerId: '00000000-0000-0000-0000-000000000001',
      kind: 'TIMER_KIND_FOLLOW_UP',
      origin: 'TIMER_ORIGIN_AGENT',
      fireAt: '2026-03-29T01:00:00Z',
      note: 'check the reply',
    });
    expect(msg.kind).toBe(TimerKind.FOLLOW_UP);
    expect(msg.origin).toBe(TimerOrigin.AGENT);
    expect(msg.fireAt?.seconds).toBe(1774746000n);
    expect(msg.cron).toBeUndefined();
  });

  it('parses the schedule TimerFired golden', () => {
    const msg = fromJson(TimerFiredSchema, {
      timerId: '00000000-0000-0000-0000-000000000001',
      kind: 'TIMER_KIND_SCHEDULE',
      origin: 'TIMER_ORIGIN_SYSTEM',
      fireAt: '2026-03-29T01:00:00Z',
      note: 'check the reply',
      cron: '0 9 * * *',
      tz: 'Europe/Berlin',
    });
    expect(msg.kind).toBe(TimerKind.SCHEDULE);
    expect(msg.cron).toBe('0 9 * * *');
    expect(msg.tz).toBe('Europe/Berlin');
  });

  it('is not in the root barrel', async () => {
    expect('OutboxMessageSchema' in (await import('./index.js'))).toBe(false);
  });
});
