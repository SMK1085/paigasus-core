// SPDX-License-Identifier: Apache-2.0
import { create } from '@bufbuild/protobuf';
import { describe, expect, it } from 'vitest';
import { OutboxMessageSchema } from './agents';

describe('agents/v1 types', () => {
  it('creates an OutboxMessage', () => {
    const msg = create(OutboxMessageSchema, { runPrn: 'p', text: 'hi' });
    expect(msg.runPrn).toBe('p');
    expect(msg.text).toBe('hi');
  });

  it('has the agents type name', () => {
    expect(OutboxMessageSchema.typeName).toBe('paigasus.agents.v1.OutboxMessage');
  });

  it('is not in the root barrel', async () => {
    expect('OutboxMessageSchema' in (await import('./index.js'))).toBe(false);
  });
});
