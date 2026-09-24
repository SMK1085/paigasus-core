// SPDX-License-Identifier: Apache-2.0
//
// The playground (SMA-635 spec § 6.3). CLIENT component. The conversation lives in React state
// only; nothing is stored. Each turn sends the full history, a stopped turn's partial answer stays
// in it, and a failed turn with no content is not sent again. The browser never holds a token: it
// posts to the zone's own route handler, which holds the session.
'use client';

import { useEffect, useRef, useState, type FormEvent, type ReactElement } from 'react';
import { Input, Label, PRIMARY_BUTTON_CLASS, SECONDARY_BUTTON_CLASS } from '@paigasus/ui';
import { createChatStreamParser, type ChatStreamError } from '../../lib/chat-stream';
import type { ComposerNotice } from './playground-notice';

/** The route under the zone's basePath: a browser fetch does not add the basePath. */
export const CHAT_PATH = '/gateway/api/chat';
/** D10: a person needs gateway_user at the org, granted out of band. */
export const MISSING_ROLE_TEXT = 'You need the gateway_user role on this organization. Ask an organization admin to grant it.';
const UNREACHABLE_TEXT = 'The console could not reach the chat route.';

type Role = 'user' | 'assistant';
type TurnStatus = 'streaming' | 'done' | 'stopped' | 'failed';
type Turn = { readonly id: number; readonly role: Role; readonly content: string; readonly status: TurnStatus };
type ShownError = { readonly message: string; readonly correlationId: string | null };

function shown(message: string, correlationId: string | null, reason: string | null): ShownError {
  return { message: reason === 'insufficient-permissions' ? MISSING_ROLE_TEXT : message, correlationId };
}

async function errorOfResponse(response: Response): Promise<ShownError> {
  try {
    const body = (await response.json()) as { error?: { message?: unknown; correlationId?: unknown; rawReason?: unknown } };
    const error = body.error ?? {};
    return shown(
      typeof error.message === 'string' ? error.message : `HTTP ${String(response.status)}`,
      typeof error.correlationId === 'string' ? error.correlationId : null,
      typeof error.rawReason === 'string' ? error.rawReason : null,
    );
  } catch {
    return { message: `HTTP ${String(response.status)}`, correlationId: null };
  }
}

function fromStream(error: ChatStreamError): ShownError {
  return shown(error.message, error.correlationId, error.reason);
}

export function Playground({ orgId, notice }: { readonly orgId: string; readonly notice: ComposerNotice }): ReactElement {
  const [model, setModel] = useState('');
  const [draft, setDraft] = useState('');
  const [turns, setTurns] = useState<readonly Turn[]>([]);
  const [error, setError] = useState<ShownError | null>(null);
  const [running, setRunning] = useState(false);
  const controllerRef = useRef<AbortController | null>(null);
  const nextIdRef = useRef(0);

  // A client-side navigation away aborts the running turn, so the upstream stops too.
  useEffect(() => () => controllerRef.current?.abort(), []);

  const update = (id: number, change: (turn: Turn) => Turn): void => {
    setTurns((all) => all.map((turn) => (turn.id === id ? change(turn) : turn)));
  };

  async function send(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (!notice.enabled || running || model.trim() === '' || draft.trim() === '') return;
    const history = turns.filter((turn) => !(turn.status === 'failed' && turn.content === '')).map((turn) => ({ role: turn.role, content: turn.content }));
    const question: Turn = { id: nextIdRef.current++, role: 'user', content: draft, status: 'done' };
    const answer: Turn = { id: nextIdRef.current++, role: 'assistant', content: '', status: 'streaming' };
    setTurns((all) => [...all, question, answer]);
    setDraft('');
    setError(null);
    setRunning(true);
    const controller = new AbortController();
    controllerRef.current = controller;
    const fail = (problem: ShownError): void => {
      setError(problem);
      update(answer.id, (turn) => ({ ...turn, status: 'failed' }));
    };
    try {
      const response = await fetch(CHAT_PATH, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ org: orgId, model: model.trim(), messages: [...history, { role: 'user', content: question.content }] }),
        signal: controller.signal,
      });
      if (!response.ok || response.body === null) {
        fail(await errorOfResponse(response));
        return;
      }
      const parser = createChatStreamParser();
      const reader = response.body.getReader();
      for (;;) {
        const { done, value } = await reader.read();
        const events = done ? parser.end() : parser.push(value);
        let over = done;
        for (const item of events) {
          if (item.kind === 'delta') {
            update(answer.id, (turn) => ({ ...turn, content: turn.content + item.text }));
          } else {
            over = true;
            if (item.kind === 'done') update(answer.id, (turn) => ({ ...turn, status: 'done' }));
            else fail(fromStream(item.error));
          }
        }
        if (over) {
          if (!done) await reader.cancel();
          break;
        }
      }
    } catch {
      if (controller.signal.aborted) update(answer.id, (turn) => ({ ...turn, status: 'stopped' }));
      else fail({ message: UNREACHABLE_TEXT, correlationId: null });
    } finally {
      if (controllerRef.current === controller) controllerRef.current = null;
      setRunning(false);
    }
  }

  const disabled = !notice.enabled;
  return (
    <section className="flex flex-col gap-4" data-testid="playground">
      {notice.enabled ? null : (
        <p role="status" data-testid="composer-notice">
          {notice.text}
        </p>
      )}
      <ol className="flex flex-col gap-2">
        {turns.map((turn) => (
          <li key={turn.id} data-testid={turn.role === 'user' ? 'user-turn' : 'assistant-turn'} data-status={turn.status} className="rounded border p-2">
            <p data-testid="turn-text" className="whitespace-pre-wrap">
              {turn.content}
            </p>
            {turn.status === 'stopped' ? <p className="text-xs">Stopped</p> : null}
          </li>
        ))}
      </ol>
      {error === null ? null : (
        <div role="alert" data-testid="playground-error" data-correlation-id={error.correlationId ?? ''}>
          <p>{error.message}</p>
          {error.correlationId === null ? null : (
            <p className="text-xs">
              Reference: <code>{error.correlationId}</code>
            </p>
          )}
        </div>
      )}
      <form aria-label="Chat composer" className="flex flex-col gap-2" onSubmit={(event) => void send(event)}>
        <Label htmlFor="playground-model">Model</Label>
        <Input id="playground-model" value={model} onChange={(event) => setModel(event.target.value)} required disabled={disabled} />
        <Label htmlFor="playground-message">Message</Label>
        <textarea id="playground-message" className="rounded border p-2" rows={3} value={draft} onChange={(event) => setDraft(event.target.value)} disabled={disabled} />
        <div className="flex gap-2">
          <button type="submit" className={PRIMARY_BUTTON_CLASS} disabled={disabled || running || model.trim() === ''}>
            Send
          </button>
          <button type="button" className={SECONDARY_BUTTON_CLASS} disabled={!running} onClick={() => controllerRef.current?.abort()}>
            Stop
          </button>
        </div>
      </form>
    </section>
  );
}
