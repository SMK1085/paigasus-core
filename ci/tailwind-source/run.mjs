// SPDX-License-Identifier: Apache-2.0

/*
 * SMA-503 AC 3 — assert a production console build still emits @paigasus/ui's CSS.
 *
 * THIS FILE MUST STAY OUTSIDE ts/apps/iam-console/. See README.md for why: that
 * directory is Tailwind's scan root, and a copy of the sentinel there would generate the very
 * utility this script asserts on.
 */

import { existsSync, readFileSync, readdirSync, mkdtempSync, writeFileSync, mkdirSync, rmSync } from 'node:fs';
import { join, dirname, resolve, sep } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
const CONSOLE_DIR = join(REPO_ROOT, 'ts', 'apps', 'iam-console');

const PROBE_SOURCE = '--paigasus' + '-ui-source-probe';
const PROBE_TOKEN = '--paigasus' + '-token-probe';

/*
 * Directories the walk below never descends into. `.next` is build output and `node_modules`
 * is installed dependencies; Tailwind ignores both, and neither is a "console source".
 */
const APP_SCAN_EXCLUDE = new Set(['.next', 'node_modules']);

/**
 * Every file that counts as "the console's own sources" for assertion 3.
 *
 * This walks the WHOLE console directory, because that is the whole of Tailwind's scan root
 * (SMA-503 fix round 2, item 3). It previously listed `app/` plus four named config files,
 * which reached 8 of the 11 tracked files there and missed `moon.yml`, `next-env.d.ts` and
 * `.prettierignore`. Tailwind extracts class candidates from every non-ignored text file in
 * the root, so a sentinel written into any of those three would generate the very utility
 * assertion 1 asserts on — and the console's own `moon.yml` already discusses this gate at
 * length, so documenting the sentinel there was a realistic way to make the guard pass with
 * the `@source` line deleted. An allowlist can only ever be as complete as the last person to
 * add a file; the walk cannot fall behind.
 */
function walk(dir, out = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (APP_SCAN_EXCLUDE.has(entry.name)) continue;
    const full = join(dir, entry.name);
    if (entry.isDirectory()) walk(full, out);
    else out.push(full);
  }
  return out;
}

/**
 * Resolve the CSS files of the CURRENT build.
 *
 * Two things make this harder than a glob.
 *
 * MEASURED in Task 1: Next 16.3.4 uses Turbopack, which writes CSS under
 * .next/static/chunks/, NOT .next/static/css/. A hardcoded directory therefore matches
 * nothing and the gate fails OPEN — which is exactly why assertion 4 ("at least one CSS file
 * was read") is load-bearing rather than a formality.
 *
 * And staleness: Next writes content-hashed CSS, Moon hydrates `outputs: ['.next']` from a
 * tarball on a cache hit, and CI restores .moon/cache across runs — so a file from an earlier
 * build can sit beside the current one and satisfy a walk while the current build has lost the
 * probe. The build manifest names only what THIS build produced, so it is the primary
 * resolver; the walk is the fallback, and the fallback is safe only because the build task
 * removes .next/static first (see the M6 step).
 */
function walkCss(dir, out = []) {
  if (!existsSync(dir)) return out;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) walkCss(full, out);
    else if (entry.name.endsWith('.css')) out.push(full);
  }
  return out;
}

function currentBuildCssFiles(nextDir) {
  const manifestPath = join(nextDir, 'app-build-manifest.json');
  if (existsSync(manifestPath)) {
    const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
    const files = new Set();
    for (const assets of Object.values(manifest.pages ?? {})) {
      for (const asset of assets) {
        const full = join(nextDir, asset);
        if (asset.endsWith('.css') && existsSync(full)) files.add(full);
      }
    }
    if (files.size > 0) return [...files];
  }
  // Fallback: the manifest is absent or names no CSS (Turbopack may not list it). Walk the
  // whole of .next/static rather than one hardcoded directory.
  const walked = walkCss(join(nextDir, 'static'));
  if (walked.length === 0) {
    throw new Error(`no CSS found under ${join(nextDir, 'static')} and no CSS in ${manifestPath} — was \`next build\` run?`);
  }
  return walked;
}

/**
 * The verdict function. Returns an array of failure messages; empty means pass.
 * Split out from the real run so --self-test can drive it over fixtures.
 */
export function verdict({ cssFiles, appFiles, readFile }) {
  const failures = [];

  // Assertion 4 first: an empty file set must fail loudly, not pass quietly.
  if (cssFiles.length === 0) {
    failures.push('no CSS file was read — the build produced none, or the manifest named none');
    return failures;
  }

  const css = cssFiles.map(readFile).join('\n');
  if (!css.includes(PROBE_SOURCE)) {
    failures.push(`sentinel A (${PROBE_SOURCE}) is absent from the built CSS — the app's Tailwind \`@source\` line no longer covers ts/packages/paigasus-ui/src`);
  }
  if (!css.includes(PROBE_TOKEN)) {
    failures.push(`sentinel B (${PROBE_TOKEN}) is absent from the built CSS — the app's \`@import '@paigasus/ui/styles.css'\` did not resolve`);
  }

  // Mirrors assertion 4's empty-set guard: an empty appFiles set must fail loudly, not let
  // the loop below pass vacuously (e.g. if the console directory is renamed).
  if (appFiles.length === 0) {
    failures.push('no console source file was scanned — ts/apps/iam-console may have been renamed or emptied, so this assertion would otherwise pass vacuously');
  }

  for (const file of appFiles) {
    const text = readFile(file);
    if (text.includes(PROBE_SOURCE) || text.includes(PROBE_TOKEN)) {
      failures.push(`${file} contains a sentinel — the console's own sources must not, or the assertion passes without the package`);
    }
  }

  return failures;
}

function realRun() {
  const nextDir = join(CONSOLE_DIR, '.next');
  const cssFiles = currentBuildCssFiles(nextDir);

  const appFiles = existsSync(CONSOLE_DIR) ? walk(CONSOLE_DIR) : [];

  const failures = verdict({ cssFiles, appFiles, readFile: (f) => readFileSync(f, 'utf8') });
  if (failures.length > 0) {
    for (const f of failures) console.error(`FAIL: ${f}`);
    console.error('== tailwind-source guard FAILED ==');
    process.exit(1);
  }
  console.log(`tailwind-source guard: both sentinels present across ${String(cssFiles.length)} CSS file(s)`);
}

function selfTest() {
  const dir = mkdtempSync(join(tmpdir(), 'tailwind-source-'));
  let checks = 0;
  const expect = (label, actual, wanted) => {
    checks += 1;
    if (actual !== wanted) {
      console.error(`SELF-TEST FAIL: ${label} — expected ${String(wanted)} failure(s), got ${String(actual)}`);
      process.exit(2);
    }
  };
  try {
    const good = join(dir, 'good.css');
    writeFileSync(good, `:root{${PROBE_TOKEN}:1}\n.x{${PROBE_SOURCE}:1}\n`);
    const noSource = join(dir, 'no-source.css');
    writeFileSync(noSource, `:root{${PROBE_TOKEN}:1}\n`);
    const noToken = join(dir, 'no-token.css');
    writeFileSync(noToken, `.x{${PROBE_SOURCE}:1}\n`);
    const cleanApp = join(dir, 'page.tsx');
    writeFileSync(cleanApp, 'export default function Page() { return null; }\n');
    const dirtyApp = join(dir, 'dirty.tsx');
    writeFileSync(dirtyApp, `const leak = '${PROBE_SOURCE}';\n`);

    const read = (f) => readFileSync(f, 'utf8');
    expect('a good build passes', verdict({ cssFiles: [good], appFiles: [cleanApp], readFile: read }).length, 0);
    expect('a missing sentinel A fails', verdict({ cssFiles: [noSource], appFiles: [cleanApp], readFile: read }).length, 1);
    expect('a missing sentinel B fails', verdict({ cssFiles: [noToken], appFiles: [cleanApp], readFile: read }).length, 1);
    expect('an empty CSS set fails', verdict({ cssFiles: [], appFiles: [cleanApp], readFile: read }).length, 1);
    expect('a sentinel in the app fails', verdict({ cssFiles: [good], appFiles: [dirtyApp], readFile: read }).length, 1);
    expect('both sentinels missing fails twice', verdict({ cssFiles: [cleanApp], appFiles: [cleanApp], readFile: read }).length, 2);
    expect('an empty appFiles set fails', verdict({ cssFiles: [good], appFiles: [], readFile: read }).length, 1);

    /*
     * The three below drive `walk()` itself, not only `verdict()` — because the defect fixed in
     * SMA-503 fix round 2 item 3 lived in WHAT WAS SCANNED, not in the verdict. The old
     * constants were `APP_SCAN_ROOTS = ['app']` plus four named config files, so `moon.yml`,
     * `next-env.d.ts` and `.prettierignore` at the console root were never read. Tailwind reads
     * all three, so a sentinel written into any of them generates the utility assertion 1
     * asserts on, and the guard would pass with the `@source` line deleted.
     */
    const fakeConsole = join(dir, 'console');
    mkdirSync(join(fakeConsole, 'app'), { recursive: true });
    mkdirSync(join(fakeConsole, '.next', 'static'), { recursive: true });
    mkdirSync(join(fakeConsole, 'node_modules', 'pkg'), { recursive: true });
    writeFileSync(join(fakeConsole, 'app', 'page.tsx'), 'export default function Page() { return null; }\n');
    writeFileSync(join(fakeConsole, 'next.config.ts'), 'export default {};\n');
    // Build output and installed dependencies BOTH carry the sentinel here. Tailwind ignores
    // them, so the walk must too — otherwise every run after a real build fails on its own
    // output, and the exclusions would be untested.
    writeFileSync(join(fakeConsole, '.next', 'static', 'old.css'), `.x{${PROBE_SOURCE}:1}\n`);
    writeFileSync(join(fakeConsole, 'node_modules', 'pkg', 'index.js'), `const x = '${PROBE_SOURCE}';\n`);

    expect('a clean console tree passes the walk', verdict({ cssFiles: [good], appFiles: walk(fakeConsole), readFile: read }).length, 0);

    // The leak the old allowlist could not reach: a sentinel documented in the console's own
    // moon.yml, at the console root, outside app/ and outside the four named config files.
    writeFileSync(join(fakeConsole, 'moon.yml'), `# this gate asserts on ${PROBE_SOURCE}\n`);
    expect('the walk catches a leak in moon.yml, which the old allowlist missed', verdict({ cssFiles: [good], appFiles: walk(fakeConsole), readFile: read }).length, 1);

    expect(
      'the walk descends into neither .next nor node_modules',
      walk(fakeConsole).filter((f) => f.includes(`${sep}.next${sep}`) || f.includes(`${sep}node_modules${sep}`)).length,
      0,
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
  console.log(`tailwind-source self-test: ${String(checks)} checks passed`);
}

function negativeControl() {
  const dir = mkdtempSync(join(tmpdir(), 'tailwind-source-neg-'));
  try {
    // Mirrors the real Turbopack layout measured in Task 1: CSS under static/chunks/.
    const nextDir = join(dir, '.next');
    mkdirSync(join(nextDir, 'static', 'chunks'), { recursive: true });
    writeFileSync(join(nextDir, 'static', 'chunks', 'a.css'), 'body{color:red}\n');
    writeFileSync(join(nextDir, 'app-build-manifest.json'), JSON.stringify({ pages: { '/page': ['static/chunks/a.css'] } }));
    const cssFiles = currentBuildCssFiles(nextDir);
    // A non-empty, clean appFiles is required here: an empty appFiles set now fails on its own
    // (see the guard in verdict()), which would inflate this count past the 2 sentinel failures
    // this control targets.
    const cleanApp = join(dir, 'page.tsx');
    writeFileSync(cleanApp, 'export default function Page() { return null; }\n');
    const failures = verdict({ cssFiles, appFiles: [cleanApp], readFile: (f) => readFileSync(f, 'utf8') });
    if (failures.length !== 2) {
      console.error(`NEGATIVE CONTROL FAIL: expected 2 failures on probe-free CSS, got ${String(failures.length)}`);
      process.exit(2);
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
  console.log('tailwind-source negative control: reported red as expected');
}

const mode = process.argv[2];
if (mode === '--self-test') selfTest();
else if (mode === '--negative-control') negativeControl();
else if (mode === undefined) realRun();
else {
  console.error(`unknown mode: ${mode}`);
  process.exit(2);
}
