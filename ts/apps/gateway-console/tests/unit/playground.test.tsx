// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// The playground client component (SMA-635 spec § 6.3). fetch is stubbed; the stream is a real
// ReadableStream, so the component's reader and the pure parser run for real.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { CHAT_PATH, MISSING_ROLE_TEXT, Playground } from '../../app/_components/playground';

const ORG = '0190a100-0000-7000-8000-0000000000e1';
const ENABLED = { enabled: true } as const;
const delta = (text: string): string => `data: ${JSON.stringify({ choices: [{ delta: { content: text } }] })}\n\n`;

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function sse(chunks: readonly string[]): Response {
  const encoder = new TextEncoder();
  return new Response(
    new ReadableStream<Uint8Array>({
      start(controller) {
        for (const chunk of chunks) controller.enqueue(encoder.encode(chunk));
        controller.close();
      },
    }),
    { status: 200, headers: { 'content-type': 'text/event-stream' } },
  );
}

async function send(text = 'hi'): Promise<void> {
  const user = userEvent.setup();
  await user.type(screen.getByLabelText('Model'), 'gpt-e2e');
  await user.type(screen.getByLabelText('Message'), text);
  await user.click(screen.getByRole('button', { name: 'Send' }));
}

describe('Playground', () => {
  it('disables the composer and shows the notice when the notice says so', () => {
    render(<Playground orgId={ORG} notice={{ enabled: false, text: 'Streaming is off on this gateway.' }} />);
    expect(screen.getByTestId('composer-notice').textContent).toBe('Streaming is off on this gateway.');
    expect(screen.getByLabelText<HTMLTextAreaElement>('Message').disabled).toBe(true);
    expect(screen.getByRole<HTMLButtonElement>('button', { name: 'Send' }).disabled).toBe(true);
  });

  it('keeps Send disabled while the model field is empty', () => {
    render(<Playground orgId={ORG} notice={ENABLED} />);
    expect(screen.getByRole<HTMLButtonElement>('button', { name: 'Send' }).disabled).toBe(true);
  });

  it('posts org, model and the history to the route, and streams the answer', async () => {
    const fetchMock = vi.fn<(url: string, init?: RequestInit) => Promise<Response>>(() => Promise.resolve(sse([delta('Hello'), delta(' world'), 'data: [DONE]\n\n'])));
    vi.stubGlobal('fetch', fetchMock);
    render(<Playground orgId={ORG} notice={ENABLED} />);
    await send();
    const answer = await screen.findByTestId('assistant-turn');
    await waitFor(() => expect(answer.getAttribute('data-status')).toBe('done'));
    expect(answer.querySelector('[data-testid="turn-text"]')?.textContent).toBe('Hello world');
    const [url, init] = fetchMock.mock.calls[0] ?? [];
    expect(url).toBe(CHAT_PATH);
    expect(JSON.parse(init?.body as string)).toEqual({ org: ORG, model: 'gpt-e2e', messages: [{ role: 'user', content: 'hi' }] });
  });

  it('names the missing role for insufficient-permissions, with the correlation id', async () => {
    vi.stubGlobal('fetch', () => Promise.resolve(Response.json({ error: { message: 'no', rawReason: 'insufficient-permissions', correlationId: 'cid-1' } }, { status: 403 })));
    render(<Playground orgId={ORG} notice={ENABLED} />);
    await send();
    const error = await screen.findByTestId('playground-error');
    expect(error.textContent).toContain(MISSING_ROLE_TEXT);
    expect(error.getAttribute('data-correlation-id')).toBe('cid-1');
  });

  it('falls back to the paigasus-correlation-id header for a non-JSON non-2xx answer', async () => {
    vi.stubGlobal('fetch', () => Promise.resolve(new Response('not json', { status: 502, headers: { 'paigasus-correlation-id': 'header-cid' } })));
    render(<Playground orgId={ORG} notice={ENABLED} />);
    await send();
    const error = await screen.findByTestId('playground-error');
    expect(error.getAttribute('data-correlation-id')).toBe('header-cid');
  });

  it('falls back to the paigasus-correlation-id header for the incomplete-stream error', async () => {
    const encoder = new TextEncoder();
    vi.stubGlobal('fetch', () =>
      Promise.resolve(
        new Response(
          new ReadableStream<Uint8Array>({
            start(controller) {
              controller.enqueue(encoder.encode(delta('partial')));
              controller.close();
            },
          }),
          { status: 200, headers: { 'content-type': 'text/event-stream', 'paigasus-correlation-id': 'header-cid-2' } },
        ),
      ),
    );
    render(<Playground orgId={ORG} notice={ENABLED} />);
    await send();
    const error = await screen.findByTestId('playground-error');
    expect(error.getAttribute('data-correlation-id')).toBe('header-cid-2');
  });

  it('Stop keeps the partial answer and marks it stopped', async () => {
    const encoder = new TextEncoder();
    vi.stubGlobal('fetch', (_url: string, init?: RequestInit) => {
      const body = new ReadableStream<Uint8Array>({
        start(controller) {
          controller.enqueue(encoder.encode(delta('partial')));
          init?.signal?.addEventListener('abort', () => controller.error(new DOMException('aborted', 'AbortError')));
        },
      });
      return Promise.resolve(new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } }));
    });
    render(<Playground orgId={ORG} notice={ENABLED} />);
    await send();
    const answer = await screen.findByTestId('assistant-turn');
    await waitFor(() => expect(answer.querySelector('[data-testid="turn-text"]')?.textContent).toBe('partial'));
    await userEvent.setup().click(screen.getByRole('button', { name: 'Stop' }));
    await waitFor(() => expect(answer.getAttribute('data-status')).toBe('stopped'));
    expect(answer.querySelector('[data-testid="turn-text"]')?.textContent).toBe('partial');
  });

  it('does not send a failed empty turn, or the user message that started it, again', async () => {
    const fetchMock = vi.fn<(url: string, init?: RequestInit) => Promise<Response>>(() =>
      Promise.resolve(Response.json({ error: { message: 'down', rawReason: null, correlationId: null } }, { status: 502 })),
    );
    vi.stubGlobal('fetch', fetchMock);
    render(<Playground orgId={ORG} notice={ENABLED} />);
    await send('first');
    await screen.findByTestId('playground-error');
    const user = userEvent.setup();
    await user.type(screen.getByLabelText('Message'), 'second');
    await user.click(screen.getByRole('button', { name: 'Send' }));
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
    const second = JSON.parse(fetchMock.mock.calls[1]?.[1]?.body as string) as { messages: { role: string; content: string }[] };
    // The failed assistant turn AND the user turn that started it are both dropped: history never
    // holds two user messages in a row (rule b). Fails if only the assistant turn were dropped,
    // since the array would then still hold 'first'.
    expect(second.messages).toEqual([{ role: 'user', content: 'second' }]);
  });

  it('drops an empty stopped turn from history, but keeps its user message', async () => {
    let seenInit: RequestInit | undefined;
    const fetchMock = vi.fn<(url: string, init?: RequestInit) => Promise<Response>>((_url, init) => {
      seenInit = init;
      return new Promise<Response>((_resolve, reject) => {
        init?.signal?.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')));
      });
    });
    vi.stubGlobal('fetch', fetchMock);
    render(<Playground orgId={ORG} notice={ENABLED} />);
    await send('first');
    await waitFor(() => expect(seenInit).toBeDefined());
    await userEvent.setup().click(screen.getByRole('button', { name: 'Stop' }));
    const answer = await screen.findByTestId('assistant-turn');
    await waitFor(() => expect(answer.getAttribute('data-status')).toBe('stopped'));
    expect(answer.querySelector('[data-testid="turn-text"]')?.textContent).toBe('');

    const user = userEvent.setup();
    await user.type(screen.getByLabelText('Message'), 'second');
    await user.click(screen.getByRole('button', { name: 'Send' }));
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
    const second = JSON.parse(fetchMock.mock.calls[1]?.[1]?.body as string) as { messages: { role: string; content: string }[] };
    // Only the empty stopped ASSISTANT turn is dropped (rule a); the user turn that started it
    // stays. Fails if the assistant turn were kept, since a stray `{ role: 'assistant', content:
    // '' }` entry would then appear between 'first' and 'second'.
    expect(second.messages).toEqual([
      { role: 'user', content: 'first' },
      { role: 'user', content: 'second' },
    ]);
  });

  it('aborts a running turn when it unmounts', async () => {
    let seen: AbortSignal | undefined;
    vi.stubGlobal('fetch', (_url: string, init?: RequestInit) => {
      seen = init?.signal ?? undefined;
      return new Promise<Response>(() => undefined);
    });
    const view = render(<Playground orgId={ORG} notice={ENABLED} />);
    await send();
    await waitFor(() => expect(seen).toBeDefined());
    view.unmount();
    expect(seen?.aborted).toBe(true);
  });
});
