// SPDX-License-Identifier: Apache-2.0
//
// The frame of the service-accounts section (SMA-636 spec § 4.6, § 5.4). CLIENT component. It owns
// ONE result region for every action of the section, the TokenPanel, and the code that runs an
// action. The server block renders it for EVERY section view kind (ok, error, denied), with
// `key={ownerPrn}` and the section body as children.
//
// WHY A FRAME. A successful issue revalidates the page (plan SPEC DEVIATION 8). The revalidated
// render can turn the section into a SectionError or a denial. That replaces the section body, but
// not this frame, so the TokenPanel and its token stay (rule 3). A move to another owner starts a
// new, empty frame.
//
// WHY ONE REGION. A row or panel control that succeeds often unmounts itself: a revoked key loses
// its Revoke button, an archived account loses every control. So every result, success or error,
// goes to this region, and no control holds the only copy of its result.
//
// HOW AN ACTION RUNS. A control hands its form to the section, which calls a runner of this frame.
// The runner calls the action DIRECTLY in a transition, with `null` as the previous state. § 5.4
// rule 2 requires that for the issue action, because useActionState sends the previous state — the
// token — back to the server; the other actions follow the same path. A newer submission replaces
// the result of an older one (a generation counter), EXCEPT an issue result: a token that IAM
// minted is always shown (rule 4). While any action runs, every submit in the section is disabled
// (rule 5 needs that for an issue).
//
// A REJECTED ACTION. A network drop or a server fault rejects the action's promise. The runner
// catches it and puts a client-built error into the result region. It never rethrows: a rethrow
// reaches the (console) error boundary, which unmounts this frame and an open TokenPanel (rule 6).
'use client';

import { createContext, use, useRef, useState, useTransition, type ReactElement, type ReactNode } from 'react';
import { ZoneLink } from '@paigasus/app-shell';
import type { FormAction } from '@paigasus/console-core';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { linkHref } from '../../lib/paging';
import { serviceAccountIdOf } from '../(console)/service-accounts/service-account-id';
import type { CreateAction, IssueKeyAction, SectionResult, SimpleControl } from '../(console)/service-accounts/view';
import { FormError } from './form-error';
import { TokenPanel, type TokenPanelHandle } from './token-panel';

// An issue success never reaches ResultMessage's success branch: runIssue below hands a successful
// token straight to the TokenPanel and never calls setResult for it. So this table excludes
// 'issue' (controller F9).
type SuccessControl = Exclude<SimpleControl, 'issue'>;

const SUCCESS_TEXT: Readonly<Record<SuccessControl, string>> = {
  allow: 'Model calls allowed.',
  revoke: 'Key revoked.',
  archive: 'Service account archived.',
};

/** § 5.3: a generic answer to "Allow model calls" can be a duplicate grant (§ 3.2). */
const ALLOW_MAY_ALREADY = 'Model calls may already be allowed. The page was reloaded.';
/** § 5.4: a plain denial and IAM's D15 check both answer forbidden. */
const ISSUE_FORBIDDEN = 'You need permission to issue keys here and to grant every role this account holds.';
/** The action's promise rejected: no answer came back. The action can still have run on the server. */
const UNREACHED_TEXT = 'The request did not reach the server. Reload the page and check the result.';

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

function overrideFor(control: SimpleControl, error: PaigasusError): string | undefined {
  if (control === 'allow' && error.presentation === 'generic') return ALLOW_MAY_ALREADY;
  if (control === 'issue' && error.presentation === 'forbidden') return ISSUE_FORBIDDEN;
  return undefined;
}

function ResultMessage({ result, path, saOffset }: { readonly result: SectionResult; readonly path: string; readonly saOffset: number }): ReactElement | null {
  if (result === null) return null;
  if (result.control === 'unreached') return <FormError error={result.error} message={UNREACHED_TEXT} />;
  if (result.control === 'token-lost') {
    return (
      <p role="alert" data-testid="token-lost" className="text-destructive text-sm font-medium">
        {`A key was issued but its token could not be shown. Revoke key ${result.prefix}.`}
      </p>
    );
  }
  if (result.control === 'create') {
    const state = result.state;
    if (state.kind === 'failed') return <FormError error={state.error} />;
    const id = serviceAccountIdOf(state.saPrn);
    const select =
      id === null ? null : (
        <ZoneLink prefetch={false} href={linkHref(path, { saOffset, sa: id })} className="underline">
          Select it
        </ZoneLink>
      );
    if (state.kind === 'partial') {
      return (
        <>
          <p role="status" className="flex flex-wrap gap-2 text-sm">
            <span>Service account created, but it cannot call models yet.</span>
            {select}
          </p>
          <FormError error={state.error} />
        </>
      );
    }
    return (
      <p role="status" className="flex flex-wrap gap-2 text-sm">
        <span>{state.granted ? 'Service account created. It can call models.' : 'Service account created. This IAM does not offer role administration, so it cannot call models from here.'}</span>
        {select}
      </p>
    );
  }
  if (result.control === 'issue') return <FormError error={result.state.error} message={overrideFor('issue', result.state.error)} />;
  if (result.state.ok) {
    return (
      <p role="status" className="text-sm">
        {SUCCESS_TEXT[result.control]}
      </p>
    );
  }
  return <FormError error={result.state.error} message={overrideFor(result.control, result.state.error)} />;
}

export type SectionRunner = {
  /** True while any action of the section runs. Every submit in the section is disabled then. */
  readonly busy: boolean;
  runCreate(action: CreateAction, form: FormData): void;
  run(control: SuccessControl, action: FormAction, form: FormData): void;
  runIssue(action: IssueKeyAction, form: FormData): void;
};

const RunnerContext = createContext<SectionRunner | null>(null);

/** The runners of the enclosing frame. The section renders only inside a ServiceAccountFrame. */
export function useSectionRunner(): SectionRunner {
  const runner = use(RunnerContext);
  if (runner === null) throw new Error('ServiceAccountSection must render inside ServiceAccountFrame.');
  return runner;
}

export type ServiceAccountFrameProps = {
  /** The page's full path, for the "Select it" link of a create result. */
  readonly path: string;
  /** The page's service-account offset, kept in the "Select it" link. */
  readonly saOffset: number;
  readonly children: ReactNode;
};

export function ServiceAccountFrame({ path, saOffset, children }: ServiceAccountFrameProps): ReactElement {
  const [result, setResult] = useState<SectionResult>(null);
  const [working, startWork] = useTransition();
  const [issuing, startIssue] = useTransition();
  const generationRef = useRef(0);
  const tokenPanelRef = useRef<TokenPanelHandle>(null);

  function next(): number {
    generationRef.current += 1;
    setResult(null);
    return generationRef.current;
  }

  const runner: SectionRunner = {
    busy: working || issuing,
    runCreate(action, form) {
      const mine = next();
      startWork(async () => {
        try {
          const state = await action(null, form);
          if (state !== null && generationRef.current === mine) setResult({ control: 'create', state });
        } catch {
          if (generationRef.current === mine) setResult({ control: 'unreached', error: unreachedError() });
        }
      });
    },
    run(control, action, form) {
      const mine = next();
      startWork(async () => {
        try {
          const state = await action(null, form);
          if (state !== null && generationRef.current === mine) setResult({ control, state });
        } catch {
          if (generationRef.current === mine) setResult({ control: 'unreached', error: unreachedError() });
        }
      });
    },
    runIssue(action, form) {
      next();
      startIssue(async () => {
        // NO generation check anywhere here (§ 5.4 rule 4): a token that IAM minted is always
        // shown, and a rejected issue may still have minted a key, so the user must hear of it.
        let state: Awaited<ReturnType<IssueKeyAction>>;
        try {
          state = await action(null, form);
        } catch {
          setResult({ control: 'unreached', error: unreachedError() });
          return;
        }
        if (state === null) return;
        if (!state.ok) {
          setResult({ control: 'issue', state });
          return;
        }
        const panel = tokenPanelRef.current;
        // Only the prefix goes to the result region, never the token.
        if (panel === null) setResult({ control: 'token-lost', prefix: state.prefix });
        else panel.show(state.token, state.prefix);
      });
    },
  };

  return (
    <RunnerContext value={runner}>
      <div className="flex flex-col gap-4">
        <div data-testid="sa-result" className="flex flex-col gap-2">
          <ResultMessage result={result} path={path} saOffset={saOffset} />
          <TokenPanel ref={tokenPanelRef} />
        </div>
        {children}
      </div>
    </RunnerContext>
  );
}
