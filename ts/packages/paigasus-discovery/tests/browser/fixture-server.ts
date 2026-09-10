// SPDX-License-Identifier: Apache-2.0
//
// A plain `node:http` server, fronted by a Vite dev server in MIDDLEWARE MODE, that renders
// app.tsx's <App> (which wraps the REAL src/disabled.tsx CapabilityDisabled — nothing here
// reimplements it) to real HTML SERVER-SIDE, then serves entry.tsx as a browser module that
// hydrates the identical tree. Static, pre-hydration markup is exactly what the JS-disabled spec
// needs to click; jsdom cannot model hit-testing at all, which is the whole reason this harness
// exists (see capability-hit-test.spec.ts's header).
//
// Vite is used PROGRAMMATICALLY, not via its CLI: it is already a transitive dependency of
// vitest, so this needs no new devDependency, and its dev-server module graph gives both
// app.tsx/entry.tsx (JSX + relative ".js"-for-".tsx" specifiers, same convention every other
// vitest config in this package already relies on) a working TS/JSX transform on demand — this
// file itself has no JSX, so Node's own `--experimental-transform-types` type-stripping is enough
// to run it directly, with no custom loader.
import http, { type Server } from 'node:http';
import { fileURLToPath } from 'node:url';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer, type ViteDevServer } from 'vite';

export interface FixtureServer {
  readonly url: string;
  close(): Promise<void>;
}

interface AppModule {
  readonly App: () => import('react').ReactElement;
}

function page(markup: string): string {
  return `<!doctype html>
<html>
<body>
<div id="root">${markup}</div>
<script type="module" src="/entry.tsx"></script>
</body>
</html>`;
}

export async function startFixtureServer(): Promise<FixtureServer> {
  const root = fileURLToPath(new URL('.', import.meta.url));

  const vite: ViteDevServer = await createServer({
    root,
    appType: 'custom',
    logLevel: 'warn',
    // No HMR: this is a short-lived, single-render test fixture, not a dev workflow — and
    // middleware mode's own docs call out that HMR needs the server wired to a websocket
    // upgrade path this harness has no use for.
    server: { middlewareMode: true, hmr: false },
    // No explicit `esbuild.jsx` override: Vite 8's default JSX transform already resolves the
    // automatic React runtime for .tsx files (MEASURED — app.tsx/entry.tsx hydrate correctly with
    // no config here at all), and `esbuild` is not an installed package in this workspace, so
    // `ESBuildOptions`'s inherited fields (jsx included) type-check as unknown here regardless.
  });

  // Rendered ONCE at startup: the component takes no props that vary per request, and every spec
  // in this suite hits the same fixed markup.
  const { App } = (await vite.ssrLoadModule('/app.tsx')) as AppModule;
  const html = page(renderToStaticMarkup(createElement(App)));

  // Synchronous: the markup is already rendered above, and `vite.middlewares` itself is a plain
  // connect-style callback, so nothing here needs to await anything.
  const httpServer: Server = http.createServer((req, res) => {
    try {
      const url = req.url ?? '/';
      if (url === '/' || url === '/index.html') {
        res.setHeader('content-type', 'text/html; charset=utf-8');
        res.end(html);
        return;
      }
      vite.middlewares(req, res, () => {
        res.statusCode = 404;
        res.end();
      });
    } catch (err) {
      console.error('browser fixture server error:', err);
      res.statusCode = 500;
      res.end('fixture server error');
    }
  });

  const port = await new Promise<number>((resolvePromise, reject) => {
    httpServer.once('error', reject);
    httpServer.listen(0, '127.0.0.1', () => {
      const address = httpServer.address();
      if (address === null || typeof address === 'string') {
        reject(new Error('fixture server has no port'));
        return;
      }
      resolvePromise(address.port);
    });
  });

  return {
    url: `http://127.0.0.1:${String(port)}/`,
    async close() {
      await new Promise<void>((resolvePromise) => httpServer.close(() => resolvePromise()));
      await vite.close();
    },
  };
}
