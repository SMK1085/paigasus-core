// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// useHydrated (SMA-636 spec § 5.4 rule 7): false in the server render, true in a client render.
import { cleanup, render, screen } from '@testing-library/react';
import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it } from 'vitest';
import { useHydrated } from '../../app/_components/use-hydrated';

afterEach(() => {
  cleanup();
});

function Probe() {
  return <p>{useHydrated() ? 'hydrated' : 'server'}</p>;
}

describe('useHydrated', () => {
  it('is false in a server render, so a JavaScript-only control is not in the HTML', () => {
    expect(renderToStaticMarkup(<Probe />)).toBe('<p>server</p>');
  });

  it('is true in a client render', () => {
    render(<Probe />);
    expect(screen.getByText('hydrated')).toBeDefined();
  });
});
