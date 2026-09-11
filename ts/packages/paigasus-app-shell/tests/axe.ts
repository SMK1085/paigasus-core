// SPDX-License-Identifier: Apache-2.0
import axe, { type Result } from 'axe-core';

/*
 * The same tags as @paigasus/ui's tests/axe.ts (SMA-503), and axe-core directly for the same
 * reason (no vitest-axe dependency tree).
 *
 * STATED LIMIT: axe cannot evaluate colour contrast in jsdom, because jsdom performs no layout.
 * These assertions cover roles, accessible names, ARIA state and labelling, not contrast. The
 * README repeats this, so a green here is not over-read.
 */
const TAGS = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'];

export type AxeOptions = {
  /**
   * Add axe's `region` rule (a best-practice rule, outside TAGS). ON for a rendered shell: the
   * shell IS the page's landmark structure (spec § 10.2). OFF for a component rendered outside a
   * shell, where every node trips it.
   */
  readonly region?: boolean;
};

function describeViolations(violations: Result[]): string {
  const body = violations
    .map((v) => {
      const nodes = v.nodes.map((n) => `      ${n.html}`).join('\n');
      return `  [${v.impact ?? 'unknown'}] ${v.id}: ${v.help}\n    ${v.helpUrl}\n${nodes}`;
    })
    .join('\n');
  return `axe found ${String(violations.length)} violation${violations.length === 1 ? '' : 's'}:\n${body}`;
}

/**
 * Assert the rendered tree has no axe violations.
 *
 * The default target is `document.body`, NOT the React Testing Library container: Radix portals
 * menu content to document.body, so a container-scoped run would check only the trigger (F15).
 */
export async function expectNoAxeViolations(target: Element = document.body, options: AxeOptions = {}): Promise<void> {
  const results = await axe.run(target, {
    runOnly: { type: 'tag', values: TAGS },
    // axe applies `rules` on top of `runOnly`, so enabling a rule here adds it even though its tag
    // is not in TAGS. tests/axe.test.tsx proves that.
    rules: { region: { enabled: options.region === true } },
  });
  if (results.violations.length > 0) {
    throw new Error(describeViolations(results.violations));
  }
}
