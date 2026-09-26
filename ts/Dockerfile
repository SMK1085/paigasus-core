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

# The builder is digest-pinned like rs/Dockerfile's builder, and the digest is the multi-platform
# manifest-list (index) digest so the pin resolves on both images.yml legs (amd64 and arm64).
# Refresh with: PROTO_REPORTER=text crane digest node:X.Y.Z-bookworm
FROM node:24.16.0-bookworm@sha256:40ad9f3064e67d6860b4bc3fe1880b2953934fd6320ada990e45fe0efa6badd7 AS builder
ARG APP
ARG BASE_PATH
WORKDIR /build
RUN corepack enable && corepack prepare pnpm@11.3.0 --activate
COPY . /build/
# SMA-634. WORKDIR is /build, which is ts/, so @paigasus/kernel's `file:` specifiers
# (../../../rs/crates/bindings/…) resolve to /rs/crates/bindings/…. The named context supplies
# only TRACKED files, named one by one rather than as whole directories, so a local build
# artifact can never enter the image: this crate directory can hold an untracked, gitignored
# `paigasus-node-bindings.*.node` left by a prior `napi build`, which does not exist on a fresh
# CI checkout, and a whole-directory COPY would have picked it up on a machine that has one. NO
# Rust is compiled here and no compiler is in this stage. paigasus-wasm's five files are exactly
# its committed, consumer-facing artifacts (its package.json `files` list); paigasus-node-bindings
# needs only its manifest and loader glue (`index.js`/`index.d.ts`) to let pnpm RESOLVE the
# `file:` dependency — no console imports the napi entry (`@paigasus/kernel`'s `./napi` export),
# so the `.node` binary itself is never needed in this image.
COPY --from=bindings paigasus-wasm/package.json paigasus-wasm/paigasus_wasm.js \
  paigasus-wasm/paigasus_wasm_bg.js paigasus-wasm/paigasus_wasm_bg.wasm \
  paigasus-wasm/paigasus_wasm.d.ts paigasus-wasm/paigasus_wasm_bg.wasm.d.ts \
  /rs/crates/bindings/paigasus-wasm/
COPY --from=bindings paigasus-node-bindings/package.json paigasus-node-bindings/index.js \
  paigasus-node-bindings/index.d.ts \
  /rs/crates/bindings/paigasus-node-bindings/
# --frozen-lockfile is required, not optional: an unlocked install can resolve versions the
# committed ts/pnpm-lock.yaml does not name, and the published image would then not be built from
# the committed lockfile with nothing saying so.
RUN pnpm install --frozen-lockfile --filter "@paigasus/${APP}..."
RUN cd "apps/${APP}" && pnpm exec next build
RUN test -f "apps/${APP}/.next/standalone/apps/${APP}/server.js" \
  || (echo "standalone entry point missing — output: 'standalone' did not take effect" >&2; exit 1)
RUN set -eu; \
    chunks="$(find "apps/${APP}/.next/standalone/apps/${APP}/.next/server/chunks" -type f -name '*paigasus_wasm_bg*.wasm' | wc -l | tr -d ' ')"; \
    if [ "$chunks" -lt 1 ]; then \
      echo "no paigasus_wasm_bg*.wasm chunk in the standalone tree — the kernel's wasm binding was not bundled" >&2; exit 1; \
    fi
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
# The fetch carries its own 2500 ms signal (SMA-670). Docker kills the probe at --timeout=3s, and
# without the signal only undici's 300 s headersTimeout bounds a server that accepts the
# connection and never answers. assert_console_pins holds the signal below the --timeout.
RUN printf '// SPDX-License-Identifier: Apache-2.0\nconst r = await fetch(`http://127.0.0.1:${process.env.PORT ?? 3000}%s/healthz`, { signal: AbortSignal.timeout(2500) });\nprocess.exit(r.ok ? 0 : 1);\n' "$BASE_PATH" > /app/healthcheck.mjs

FROM gcr.io/distroless/nodejs24-debian12:nonroot@sha256:14d42e2511532589a7c7e01a753667a74fcc96266e137e8125006b87b0c32d0a
COPY --from=builder /app /app
ENV PORT=3000 HOSTNAME=0.0.0.0 NODE_ENV=production
EXPOSE 3000
USER 65532:65532
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=3s --start-period=30s --retries=3 \
  CMD ["/nodejs/bin/node", "/app/healthcheck.mjs"]
ENTRYPOINT ["/nodejs/bin/node", "/app/entrypoint.mjs"]
