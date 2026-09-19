// SPDX-License-Identifier: Apache-2.0
//
// The frame of /iam/dead-letters (SMA-629 spec § 6.4). CLIENT component. The page renders it for
// EVERY view that does not throw — the table, the empty list, the degraded view and a SectionError —
// keyed by `${eventType}|${cursor}`, with the view as children. The design is the gateway console's
// ServiceAccountFrame (ts/apps/gateway-console/app/_components/service-account-frame.tsx:1-27).
//
// WHY A FRAME. React 19 resets a form before every action. A successful replay or discard removes the
// row, and the revalidated render can also replace the table with an error view. A control that held
// its own result would lose it when it unmounts. So this frame holds the ONE result region, and no
// row control holds a result.
//
// HOW AN ACTION RUNS. A row form hands its FormData to the runner, which calls the Server Action
// DIRECTLY in a transition, with `null` as the previous state (not through useActionState). It
// records the id from the FormData, so the result can name it. A generation counter makes the newest
// submission's result win. While any action runs, EVERY Replay and Discard button is disabled.
//
// A REJECTED ACTION (a network drop, a server fault). The runner catches it and shows a client-built
// error. It never rethrows: a rethrow reaches (console)/error.tsx, which unmounts this frame. The
// action may still have run on the server, so the text says the result is unknown; a later retry of
// the same id answers not-found, and DEAD_LETTER_GONE covers that (§ 9).
'use client';

import { createContext, use, useRef, useState, useTransition, type FormEvent, type ReactElement, type ReactNode } from 'react';
import { PRIMARY_BUTTON_CLASS, SECONDARY_BUTTON_CLASS } from '@paigasus/ui';
import type { ActionResult, FormAction } from '@paigasus/console-core';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { DEAD_LETTER_GONE } from '../../_components/error-copy';
import { FormError } from '../../_components/form-error';

export type DeadLetterControl = 'replay' | 'discard';

export type DeadLetterActions = { readonly replay: FormAction; readonly discard: FormAction };

type FrameResult =
  null | { readonly kind: 'answer'; readonly control: DeadLetterControl; readonly id: string; readonly state: ActionResult } | { readonly kind: 'unreached'; readonly error: PaigasusError };

/** The action's promise rejected: no answer came back. The action can still have run on the server. */
export const UNREACHED_TEXT = 'No answer came back from the server, so the result is unknown. Reload the page to see the current queue.';

export function successText(control: DeadLetterControl, id: string): string {
  return control === 'replay' ? `Replayed event ${id}.` : `Discarded event ${id}.`;
}

/** § 6.4 (application/dead_letters.rs:215-229): a discard deletes the entry; the audit log keeps a copy. */
export function discardConfirmation(id: string): string {
  return `Discard event ${id}? IAM deletes it from the dead-letter queue, and it is not published again. The audit log keeps a copy of the event.`;
}

/** A client-built error for a rejected action. It never reached IAM, so it has no correlation id. */
function unreachedError(): PaigasusError {
  return {
    presentation: 'generic',
    domain: null,
    reason: null,
    rawReason: null,
    rawDomain: null,
    message: UNREACHED_TEXT,
    correlationId: null,
    requestId: null,
    retryable: null,
    metadata: {},
    transport: { kind: 'transport', cause: 'network' },
  };
}

function ResultMessage({ result }: { readonly result: FrameResult }): ReactElement | null {
  if (result === null) return null;
  if (result.kind === 'unreached') return <FormError error={result.error} message={UNREACHED_TEXT} />;
  if (result.state.ok) return <p className="text-sm">{successText(result.control, result.id)}</p>;
  const error = result.state.error;
  // § 6.5: only a not-found answer of these two actions gets the dead-letter sentence.
  return <FormError error={error} message={error.presentation === 'not-found' ? DEAD_LETTER_GONE : undefined} />;
}

type Runner = {
  /** True while any action of the frame runs. Every Replay and Discard button is disabled then. */
  readonly busy: boolean;
  run(control: DeadLetterControl, form: FormData): void;
};

const RunnerContext = createContext<Runner | null>(null);

function useRunner(): Runner {
  const runner = use(RunnerContext);
  if (runner === null) throw new Error('DeadLetterRowControls must render inside DeadLettersFrame.');
  return runner;
}

export function DeadLettersFrame({ actions, children }: { readonly actions: DeadLetterActions; readonly children: ReactNode }): ReactElement {
  const [result, setResult] = useState<FrameResult>(null);
  const [busy, startWork] = useTransition();
  const generationRef = useRef(0);

  const runner: Runner = {
    busy,
    run(control, form) {
      generationRef.current += 1;
      const mine = generationRef.current;
      const raw = form.get('id');
      const id = typeof raw === 'string' ? raw.trim() : '';
      setResult(null);
      startWork(async () => {
        try {
          const state = await actions[control](null, form);
          if (state !== null && generationRef.current === mine) setResult({ kind: 'answer', control, id, state });
        } catch {
          if (generationRef.current === mine) setResult({ kind: 'unreached', error: unreachedError() });
        }
      });
    },
  };

  return (
    <RunnerContext value={runner}>
      <div className="flex flex-col gap-4">
        <div role="status" data-testid="dead-letters-result" className="flex flex-col gap-2">
          <ResultMessage result={result} />
        </div>
        {children}
      </div>
    </RunnerContext>
  );
}

/**
 * Replay (one step: it is the normal recovery path) and Discard (two steps, the ArchiveButton
 * pattern of app/_components/lifecycle-button.tsx:42-80). No result of their own: the frame shows it.
 */
export function DeadLetterRowControls({ id }: { readonly id: string }): ReactElement {
  const runner = useRunner();
  const [confirming, setConfirming] = useState(false);

  function submit(control: DeadLetterControl) {
    return (event: FormEvent<HTMLFormElement>): void => {
      event.preventDefault();
      runner.run(control, new FormData(event.currentTarget));
      if (control === 'discard') setConfirming(false);
    };
  }

  return (
    <div data-testid={`dead-letter-controls-${id}`} className="flex flex-col gap-2">
      <form aria-label={`Replay event ${id}`} onSubmit={submit('replay')}>
        <input type="hidden" name="id" value={id} />
        <button type="submit" disabled={runner.busy} className={PRIMARY_BUTTON_CLASS}>
          Replay
        </button>
      </form>
      {confirming ? (
        <form aria-label={`Confirm the discard of event ${id}`} className="flex flex-col gap-2" onSubmit={submit('discard')}>
          <p className="text-sm">{discardConfirmation(id)}</p>
          <input type="hidden" name="id" value={id} />
          <div className="flex flex-wrap gap-2">
            <button type="submit" disabled={runner.busy} className={PRIMARY_BUTTON_CLASS}>
              Confirm discard
            </button>
            <button
              type="button"
              disabled={runner.busy}
              className={SECONDARY_BUTTON_CLASS}
              onClick={() => {
                setConfirming(false);
              }}
            >
              Cancel
            </button>
          </div>
        </form>
      ) : (
        <button
          type="button"
          disabled={runner.busy}
          className={SECONDARY_BUTTON_CLASS}
          onClick={() => {
            setConfirming(true);
          }}
        >
          Discard
        </button>
      )}
    </div>
  );
}
