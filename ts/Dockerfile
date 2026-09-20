# SPDX-License-Identifier: Apache-2.0
#
# One image definition for both console zones, selected by APP. Mirrors rs/Dockerfile's BIN arg.
#
# Exec-form ENTRYPOINT and HEALTHCHECK do NOT expand ARG or ENV. rs/Dockerfile solves that with a
# fixed binary path; this solves it with two fixed-path .mjs files the builder writes. They are
# .mjs, not .js, because every console package.json sets "type": "module" and Next copies it into
# the standalone tree — a CJS require() shim would depend on require(esm) interop and would break
# outright if a "type": "module" manifest landed at /app.
ARG APP
ARG BASE_PATH

FROM node:24.16.0-bookworm AS builder
ARG APP
ARG BASE_PATH
WORKDIR /build
RUN corepack enable && corepack prepare pnpm@11.3.0 --activate
COPY . /build/
# --frozen-lockfile is required, not optional: an unlocked install can resolve versions the
# committed ts/pnpm-lock.yaml does not name, and the published image would then not be built from
# the committed lockfile with nothing saying so.
RUN pnpm install --frozen-lockfile --filter "@paigasus/${APP}..."
RUN cd "apps/${APP}" && pnpm exec next build
RUN test -f "apps/${APP}/.next/standalone/apps/${APP}/server.js" \
  || (echo "standalone entry point missing — output: 'standalone' did not take effect" >&2; exit 1)
RUN set -eu; \
    cd "apps/${APP}"; \
    manifest_base="$(sed -n 's/.*"basePath": *"\([^"]*\)".*/\1/p' .next/routes-manifest.json | sed -n 1p)"; \
    if [ "$manifest_base" != "$BASE_PATH" ]; then \
      echo "BASE_PATH build-arg '${BASE_PATH}' does not match the compiled basePath '${manifest_base}' in .next/routes-manifest.json for ${APP} — check --build-arg BASE_PATH against apps/${APP}/next.config.ts" >&2; exit 1; \
    fi; \
    dest=".next/standalone/apps/${APP}"; \
    rm -rf "$dest/.next/static" "$dest/public"; \
    cp -R .next/static "$dest/.next/static"; \
    if [ -d public ]; then cp -R public "$dest/public"; fi; \
    build_id="$(cat .next/BUILD_ID)"; \
    staged_id="$(cat "$dest/.next/BUILD_ID" 2>/dev/null || true)"; \
    if [ -z "$build_id" ] || [ "$staged_id" != "$build_id" ] || [ ! -d "$dest/.next/static/$build_id" ]; then \
      echo "staging the standalone tree failed (BUILD_ID '$build_id', staged '$staged_id')" >&2; exit 1; \
    fi
RUN mkdir -p /app && cp -R "apps/${APP}/.next/standalone/." /app/
RUN printf "// SPDX-License-Identifier: Apache-2.0\nawait import('./apps/%s/server.js');\n" "$APP" > /app/entrypoint.mjs
RUN printf '// SPDX-License-Identifier: Apache-2.0\nconst r = await fetch(`http://127.0.0.1:${process.env.PORT ?? 3000}%s/healthz`);\nprocess.exit(r.ok ? 0 : 1);\n' "$BASE_PATH" > /app/healthcheck.mjs

FROM gcr.io/distroless/nodejs24-debian12:nonroot@sha256:14d42e2511532589a7c7e01a753667a74fcc96266e137e8125006b87b0c32d0a
COPY --from=builder /app /app
ENV PORT=3000 HOSTNAME=0.0.0.0 NODE_ENV=production
EXPOSE 3000
USER 65532:65532
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=3s --start-period=30s --retries=3 \
  CMD ["/nodejs/bin/node", "/app/healthcheck.mjs"]
ENTRYPOINT ["/nodejs/bin/node", "/app/entrypoint.mjs"]
