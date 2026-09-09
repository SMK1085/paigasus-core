// SPDX-License-Identifier: Apache-2.0
import axe, { type Result } from 'axe-core';

/*
 * AC 4 (SMA-503). axe-core is used directly rather than through vitest-axe, which last
 * published on 2025-01-22 and pulls chalk, redent, lodash-es and dom-accessibility-api for a
 * matcher that is about fifteen lines of code. This repository is public and runs an OSV
 * gate, so fewer transitive dependencies is the better trade.
 *
 * STATED LIMIT: axe cannot evaluate color-contrast in jsdom, because jsdom performs no
 * layout. These assertions cover roles, accessible names, ARIA state and labelling. They do
 * NOT cover contrast — that stays a design responsibility, decided in the token layer. The
 * package README repeats this so a green here is not over-read.
 */
const TAGS = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'];

const DISABLED_RULES = {
  // Fires on every isolated component render, because a fragment has no landmark wrapper.
  // It has no meaning outside a full page, which this tier never renders.
  region: { enabled: false },
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
 * The default target is `document.body`, NOT the container React Testing Library returns.
 * Radix Dialog, DropdownMenu and Popover render their content into a portal on document.body,
 * so a container-scoped assertion would check a subtree holding only the trigger — and the
 * three most accessibility-critical components would go green for the wrong reason.
 */
export async function expectNoAxeViolations(target: Element = document.body): Promise<void> {
  const results = await axe.run(target, {
    runOnly: { type: 'tag', values: TAGS },
    rules: DISABLED_RULES,
  });
  if (results.violations.length > 0) {
    throw new Error(describeViolations(results.violations));
  }
}
