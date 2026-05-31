// SPDX-License-Identifier: Apache-2.0
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import process from 'node:process';

// Enumerate Moon's resolved project graph (moon query projects prints JSON; the --json
// flag is rejected). Each entry has top-level id/source/root/language/tasks, and tasks
// already reflects workspace.inheritedTasks.exclude.
const { projects } = JSON.parse(execFileSync('moon', ['query', 'projects'], { encoding: 'utf8' }));
const tsProjects = projects.filter((p) => p.language === 'typescript');

// A config-only package is not a tsc compilation unit (no tsconfig.json), so the inherited
// `tsc -p tsconfig.json --noEmit` build/typecheck would fail TS5058. It must exclude them.
const violations = tsProjects.filter((p) => {
  if (existsSync(join(p.root, 'tsconfig.json'))) return false;
  const tasks = Object.keys(p.tasks ?? {});
  return tasks.includes('build') || tasks.includes('typecheck');
});

if (violations.length > 0) {
  const lines = [
    'Config-only TS packages (no tsconfig.json) must exclude the inherited build/typecheck:',
    ...violations.map((p) => `  - ${p.id} (${p.source})`),
    '',
    "Fix: add `workspace.inheritedTasks.exclude: ['build', 'typecheck']` to the project's moon.yml",
    '(scaffold archetype `config`), or add a tsconfig.json. See CONTRIBUTING "Moon project files".',
  ];
  process.stderr.write(`${lines.join('\n')}\n`);
  process.exit(1);
}

process.stdout.write(`config-only guard: ${tsProjects.length} TS projects checked, no violations\n`);
