// SPDX-License-Identifier: Apache-2.0
//
// The three error boundaries (spec § 6.1): each shows fixed copy and Next's digest, never the
// error's message.
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import ConsoleError from '../../app/(console)/error';
import RootError from '../../app/error';
import GlobalError from '../../app/global-error';

const error = Object.assign(new Error('internal detail: db password rejected'), { digest: 'digest-123' });
const reset = (): void => undefined;

describe('the error boundaries', () => {
  it.each([
    ['(console)/error.tsx', () => renderToStaticMarkup(<ConsoleError error={error} reset={reset} />)],
    ['app/error.tsx', () => renderToStaticMarkup(<RootError error={error} reset={reset} />)],
    ['app/global-error.tsx', () => renderToStaticMarkup(<GlobalError error={error} />)],
  ])('%s shows the digest and hides the message', (_name, render) => {
    const html = render();
    expect(html).toContain('digest-123');
    expect(html).not.toContain('db password');
  });
});
