# SMA-513 — Console images and Helm chart (PR 1 and PR 2a)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build distroless container images for the two console zones, and a Helm chart that renders the ingress rules and both consoles' zone and service maps from one values block.

**Architecture:** One parameterized `ts/Dockerfile` serves both consoles, selected by `--build-arg APP`, mirroring `rs/Dockerfile`'s `BIN` argument. `ci/images/run.sh` gains a parallel console build and smoke path, added as new functions rather than by restructuring the Rust path. The chart keys everything off one `zones` map; a zone's `enabled` flag governs its backend, its console, its ingress rule and both environment entries together.

**Tech Stack:** Docker + BuildKit, `gcr.io/distroless/nodejs24-debian12`, Next.js 16 standalone output, pnpm 11 workspaces, Helm 3, bash.

**Spec:** `docs/superpowers/specs/2026-09-19-sma-513-multi-zone-ingress-helm-design.md`

**Not in this plan:** PR 2b (the `repo:helm-render` gate and its eight registration obligations) and PR 3 (the kind job). Those are written after PR 2a lands, because 2b's script pins name literal lines in files 2a creates and its golden files are byte-exact pins of a chart that does not exist yet.

## Global Constraints

- **Start only after PR 270 (SMA-658) merges.** It rewrites `ci/images/run.sh` and `docs/ops/RUNBOOK-containers.md`, which Tasks 3, 4 and 6 modify. Task 1 and all of PR 2a (Tasks 7–13) do not touch those files and may run earlier.
- **Every change to `ci/images/run.sh` is additive.** New functions and new dispatch arms only. Do not restructure `crate_for`, `assert_pins`, `build_one`, `smoke` or `assert_base_intact`. This keeps the merge with PR 270 tractable.
- Every source file opens with an SPDX header: `// SPDX-License-Identifier: Apache-2.0` (`#` for shell, YAML and Python).
- Branch: `feature/sma-513-ops-multi-zone-ingress-and-helm-chart`. Conventional commits with a workspace scope. The allowed scope enum is `rs, py, ts, contracts, ci, docs, deps, release, repo, claude, workspace` — **`ops` is not allowed**.
- Bash tool PATH lacks the proto-managed CLIs. Prefix commands with
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Export `PROTO_REPORTER=text` once at the top of any script that captures the stdout of a proto call or a proto-shimmed tool.
- **Never pipe into an early-exit reader** (`grep -q`, `grep -m N`, `head`, `awk … exit`) in any tracked `*.sh`. `ci/actionlint/run.sh` check 13 bans it with an empty allowlist. Use `grep -qF -- "$lit" < <(printf '%s' "$block")` or capture into a variable first.
- **No `mapfile` and no `declare -A`** in any new `ci/**/run.sh`. Both are absent from the development Mac's system `/bin/bash` 3.2.57 and turn every assertion row into a false failure.
- **No file may be named after a Windows reserved device name** — `con`, `prn`, `aux`, `nul`, `com1`–`com9`, `lpt1`–`lpt9` — at any extension. Git cannot check such a file out on Windows and only a Windows matrix job catches it.

### Exact values copied from the spec

| Thing | Value |
| -- | -- |
| Console base image | `gcr.io/distroless/nodejs24-debian12:nonroot@sha256:14d42e2511532589a7c7e01a753667a74fcc96266e137e8125006b87b0c32d0a` |
| Image uid:gid | `65532:65532` |
| Node pin (`.prototools`) | `24.16.0` |
| IAM gRPC default port | `9090` |
| IAM HTTP default port | `8080` |
| Gateway HTTP default port | `8088` |
| Console container port | `3000` |
| Zone ids | must be members of `SERVICE_SLUGS` (`ts/packages/paigasus-discovery/src/core/state.ts:21`) |
| Registry prefix | `${PAIGASUS_IMAGE_REGISTRY:-ghcr.io/smk1085}` |

---

# PR 1 — Console container images

### Task 1: Measure whether a filtered pnpm install excludes the kernel packages

The spec records this as *"a claim to verify in the first task of PR 1, not a measured fact"* (§ 5.3). `@paigasus/node-bindings` is a pnpm `file:` dependency with no `.node` binary in a fresh tree, so if a filtered install still materializes it, the Docker build fails and Task 2 needs a pruned build context instead.

**Files:**
- Create: `docs/superpowers/specs/2026-09-19-sma-513-measurements.md`

**Interfaces:**
- Produces: a recorded verdict that Task 2 reads — either "filtered install is sufficient" or "the build context must exclude `ts/packages/paigasus-kernel`".

- [ ] **Step 1: Run the filtered install into a scratch store and list what it linked**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-console-helm
SCRATCH="$(mktemp -d)"
pnpm -C ts install --frozen-lockfile --filter @paigasus/iam-console... \
  --virtual-store-dir "$SCRATCH/.pnpm" --modules-dir "$SCRATCH/node_modules" 2>&1 | tail -20
ls "$SCRATCH/node_modules/@paigasus" 2>/dev/null || echo "(no @paigasus dir)"
```

- [ ] **Step 2: Record the verdict**

Write `docs/superpowers/specs/2026-09-19-sma-513-measurements.md` with an M1 section naming the exact command, the listed packages, and one of the two verdicts. If `@paigasus/kernel` or `@paigasus/node-bindings` appears, the verdict is "the build context must exclude `ts/packages/paigasus-kernel`" and Task 2 Step 1 gains that exclusion.

State the pnpm version the measurement was taken on (`pnpm --version`), because this is a resolver behaviour and a pnpm bump can move it.

- [ ] **Step 3: Commit**

```bash
git add docs/superpowers/specs/2026-09-19-sma-513-measurements.md
git commit -m "docs(repo): measure whether a filtered pnpm install excludes the kernel packages (SMA-513)"
```

---

### Task 2: `ts/Dockerfile` — builder and runtime stages

The builder must reproduce what `ts/apps/<app>/moon.yml`'s `build` script does, because **Next's standalone output contains no `.next/static` and no `public/`** (spec F7). Without the staging copy the image returns 404 for every chunk, and a `/healthz` probe still passes.

**Files:**
- Create: `ts/Dockerfile`
- Create: `ts/.dockerignore`

**Interfaces:**
- Consumes: Task 1's verdict.
- Produces: an image buildable as
  `docker build -f ts/Dockerfile --build-arg APP=iam-console --build-arg BASE_PATH=/iam -t iam-console:dev ts`,
  serving on port 3000, running as uid 65532, with `/app/entrypoint.mjs` and `/app/healthcheck.mjs` at fixed paths.

- [ ] **Step 1: Write `ts/.dockerignore`**

```
# SPDX-License-Identifier: Apache-2.0
node_modules
**/node_modules
**/.next
**/.turbo
**/test-results
**/playwright-report
```

If Task 1's verdict was "exclude the kernel", add these two lines:

```
packages/paigasus-kernel
packages/paigasus-node-bindings
```

- [ ] **Step 2: Write the failing assertion first — a script that proves an image serves its chunks**

Create `ts/tests/docker/serves-chunks.sh`. It is the executable form of spec § 5.4 and is what Task 4 later moves into `ci/images/run.sh`. Writing it now means Task 2's Step 5 has something to fail against.

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Proves a console image serves its own static chunks. Next's standalone output has NO
# .next/static (SMA-510), so an image built without the staging copy answers 200 on
# <basePath>/healthz and 404 on every chunk. This asserts the chunk, not the probe.
set -euo pipefail

image="$1" base_path="$2" other_prefix="$3"
name="serves-chunks-$$"
trap 'docker rm -f "$name" >/dev/null 2>&1 || true' EXIT

docker run -d --name "$name" -p 0:3000 \
  -e PAIGASUS_ZONE="${base_path#/}" \
  -e PAIGASUS_ZONES="{\"iam\":\"/iam\",\"gateway\":\"/gateway\"}" \
  "$image" >/dev/null
port="$(docker port "$name" 3000/tcp)"; port="${port##*:}"
origin="http://127.0.0.1:${port}"

# Step 1: the page renders.
html="$(curl -fsS --retry 20 --retry-delay 1 --retry-all-errors "${origin}${base_path}/")"

# Step 2: extract one chunk URL. No pipe into an early-exit reader: capture, then read.
chunk="$(printf '%s' "$html" | grep -oE "${base_path}/_next/static/[^\"']+\.js" | sort -u | sed -n 1p)"
if [ -z "$chunk" ]; then
  echo "::error::${image}: no ${base_path}/_next/static/*.js URL in the rendered page" >&2
  exit 1
fi

# Step 3: the chunk is served, with a body.
bytes="$(curl -fsS "${origin}${chunk}" | wc -c | tr -d ' ')"
if [ "${bytes:-0}" -lt 1 ]; then
  echo "::error::${image}: ${chunk} served an empty body — .next/static was not staged into the image" >&2
  exit 1
fi

# Step 4: the other zone's prefix does not serve it.
code="$(curl -s -o /dev/null -w '%{http_code}' "${origin}${other_prefix}${chunk#"$base_path"}")"
if [ "$code" != "404" ]; then
  echo "::error::${image}: ${other_prefix} also served the chunk (HTTP ${code}); zone asset prefixes collide" >&2
  exit 1
fi

echo "  ${image}: serves ${chunk} (${bytes} bytes), 404 under ${other_prefix}"
```

```bash
chmod +x ts/tests/docker/serves-chunks.sh
```

- [ ] **Step 3: Write `ts/Dockerfile` with a deliberately incomplete builder**

Write it **without** the staging copy, so Step 5 fails for the real reason and proves the assertion bites.

```dockerfile
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
```

- [ ] **Step 4: Build the image**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-console-helm
docker build -f ts/Dockerfile \
  --build-arg APP=iam-console --build-arg BASE_PATH=/iam \
  -t iam-console:dev ts
```

Expected: the build succeeds. The defect is invisible at build time — that is the point.

- [ ] **Step 5: Run the assertion and verify it FAILS**

```bash
ts/tests/docker/serves-chunks.sh iam-console:dev /iam /gateway
```

Expected: FAIL at step 3 or step 2 with `.next/static was not staged into the image`, or with `no /iam/_next/static/*.js URL in the rendered page`.

**If it passes, stop.** Either Next changed its standalone behaviour or the assertion is not reaching the chunk. Do not continue until the red is understood — a green here means the control does not bite, which is the exact failure this task exists to prevent.

- [ ] **Step 6: Add the staging copy to the builder**

Insert after the `next build` assertion line and before the `cp -R` to `/app`:

```dockerfile
RUN set -eu; \
    cd "apps/${APP}"; \
    dest=".next/standalone/apps/${APP}"; \
    rm -rf "$dest/.next/static" "$dest/public"; \
    cp -R .next/static "$dest/.next/static"; \
    if [ -d public ]; then cp -R public "$dest/public"; fi; \
    build_id="$(cat .next/BUILD_ID)"; \
    staged_id="$(cat "$dest/.next/BUILD_ID" 2>/dev/null || true)"; \
    if [ -z "$build_id" ] || [ "$staged_id" != "$build_id" ] || [ ! -d "$dest/.next/static/$build_id" ]; then \
      echo "staging the standalone tree failed (BUILD_ID '$build_id', staged '$staged_id')" >&2; exit 1; \
    fi
```

This is the same sequence as `ts/apps/iam-console/moon.yml:80-99`. Task 4 Step 5 adds the assertion that keeps the two sites in agreement.

- [ ] **Step 7: Rebuild and verify the assertion PASSES**

```bash
docker build -f ts/Dockerfile --build-arg APP=iam-console --build-arg BASE_PATH=/iam -t iam-console:dev ts
ts/tests/docker/serves-chunks.sh iam-console:dev /iam /gateway
```

Expected: `iam-console:dev: serves /iam/_next/static/….js (NNNN bytes), 404 under /gateway`

- [ ] **Step 8: Repeat for the gateway zone**

```bash
docker build -f ts/Dockerfile --build-arg APP=gateway-console --build-arg BASE_PATH=/gateway -t gateway-console:dev ts
ts/tests/docker/serves-chunks.sh gateway-console:dev /gateway /iam
```

Expected: PASS.

- [ ] **Step 9: Verify the identity and the absence of a shell**

```bash
docker run -d --name t$$ -e PAIGASUS_ZONE=iam -e PAIGASUS_ZONES='{"iam":"/iam"}' iam-console:dev
docker top t$$ -o pid,uid
docker run --rm --entrypoint /bin/sh iam-console:dev -c true && echo "HAS SHELL (bad)" || echo "no shell (good)"
docker rm -f t$$
```

Expected: uid `65532`, and `no shell (good)`.

- [ ] **Step 10: Commit**

```bash
git add ts/Dockerfile ts/.dockerignore ts/tests/docker/serves-chunks.sh
git commit -m "feat(ts): container images for the two console zones (SMA-513)"
```

---

### Task 3: Console build path in `ci/images/run.sh`

Additive only. New functions beside the Rust ones, new dispatch arms.

**Files:**
- Modify: `ci/images/run.sh`

**Interfaces:**
- Consumes: `ts/Dockerfile` from Task 2.
- Produces: `app_for()`, `base_path_for()`, `assert_console_pins()`, `build_console_one()`, and the dispatch arms `build-console [iam|gateway]` and `all-consoles`. Task 4 adds `smoke_consoles()` and calls it from `all-consoles`.

- [ ] **Step 1: Re-read the file as PR 270 left it**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-console-helm
git log --oneline -3 -- ci/images/run.sh
grep -n '^[a-z_]*()' ci/images/run.sh
sed -n '1,20p' ci/images/run.sh
```

PR 270 rewrites this file. Confirm the function names and the dispatch `case` before editing, and adapt the anchors below if they moved.

- [ ] **Step 2: Write the failing assertion — `assert_console_pins` self-check**

Add near `assert_pins`:

```bash
# The console image's Node major is a SECOND pin beside .prototools. distroless publishes no
# patch-level tags, so only the major can be held equal — that is the ceiling of this check, not
# an oversight. A distroless bump that crosses a major reds here rather than shipping a runtime
# the repo does not pin.
assert_console_pins() {
  local df="$ROOT/ts/Dockerfile" want_major base_major proto_node
  proto_node="$(sed -n 's/^node = "\([0-9.]*\)"$/\1/p' "$ROOT/.prototools")"
  want_major="${proto_node%%.*}"
  base_major="$(sed -n 's#^FROM gcr\.io/distroless/nodejs\([0-9]*\)-debian12.*#\1#p' "$df")"
  if [ -z "$base_major" ]; then
    echo "::error::ts/Dockerfile: no gcr.io/distroless/nodejsNN-debian12 FROM line found." >&2
    return 1
  fi
  if [ "$base_major" != "$want_major" ]; then
    echo "::error::ts/Dockerfile pins Node ${base_major} but .prototools pins ${proto_node}." >&2
    return 1
  fi
  if grep -qE '^ENV +PAIGASUS_' "$df"; then
    echo "::error::ts/Dockerfile bakes a PAIGASUS_* env var; console config is deployment-varying and must stay runtime-only." >&2
    return 1
  fi
  if ! grep -qF -- '--frozen-lockfile' "$df"; then
    echo "::error::ts/Dockerfile installs without --frozen-lockfile; the image would not be built from the committed lockfile." >&2
    return 1
  fi
  echo "  ts/Dockerfile: Node ${base_major} matches .prototools, no baked PAIGASUS_*, --frozen-lockfile present"
}
```

- [ ] **Step 3: Prove each of the three assertions fails on a mutation**

Run each mutation, confirm the named red, then restore by **reverting the single inserted line**, never with `git checkout --` (that would also revert Task 2's uncommitted work if any remains).

```bash
# (a) Node major
sed -i.bak 's#distroless/nodejs24-debian12#distroless/nodejs22-debian12#' ts/Dockerfile
bash -c 'source ci/images/run.sh 2>/dev/null; assert_console_pins' || echo "red as expected (a)"
mv ts/Dockerfile.bak ts/Dockerfile

# (b) baked env
printf 'ENV PAIGASUS_ZONE=iam\n' >> ts/Dockerfile
bash -c 'source ci/images/run.sh 2>/dev/null; assert_console_pins' || echo "red as expected (b)"
sed -i.bak '/^ENV PAIGASUS_ZONE=iam$/d' ts/Dockerfile && rm ts/Dockerfile.bak

# (c) frozen lockfile
sed -i.bak 's/ --frozen-lockfile//' ts/Dockerfile
bash -c 'source ci/images/run.sh 2>/dev/null; assert_console_pins' || echo "red as expected (c)"
mv ts/Dockerfile.bak ts/Dockerfile
```

Expected: three reds, each naming its own cause. Then re-run once clean and expect the green summary line.

Note `mv file.bak file` rolls mtime **backwards**, which can make a later build reuse a stale layer. Run `touch ts/Dockerfile` after each restore.

- [ ] **Step 4: Add `app_for`, `base_path_for` and `build_console_one`**

```bash
app_for() {
  case "$1" in
    iam)     echo "iam-console" ;;
    gateway) echo "gateway-console" ;;
    *) echo "unknown console: $1" >&2; return 1 ;;
  esac
}

base_path_for() {
  case "$1" in
    iam)     echo "/iam" ;;
    gateway) echo "/gateway" ;;
    *) echo "unknown console: $1" >&2; return 1 ;;
  esac
}

build_console_one() {
  local service="$1" app base_path tag
  app="$(app_for "$service")"
  base_path="$(base_path_for "$service")"
  tag="${REGISTRY}/paigasus-${app}:${REVISION}"
  echo "== build ${app} =="
  docker build \
    --progress=plain \
    --load \
    -f "$ROOT/ts/Dockerfile" \
    --build-arg "APP=${app}" \
    --build-arg "BASE_PATH=${base_path}" \
    --label "org.opencontainers.image.title=paigasus-${app}" \
    --label "org.opencontainers.image.description=Paigasus ${service} console" \
    --label "org.opencontainers.image.source=https://github.com/SMK1085/paigasus-core" \
    --label "org.opencontainers.image.revision=${REVISION}" \
    --label "org.opencontainers.image.licenses=Apache-2.0" \
    -t "$tag" -t "${app}:dev" \
    "$ROOT/ts"
  echo "  built ${tag}"
}
```

There is no `--no-cache-filter` here. That flag exists on the Rust path because its `rootfs` stage is byte-identical between services and BuildKit would cache-hit it, leaving the second service's chisel manifest empty. The console builder's every stage references `APP`, so no stage is shared between the two builds and there is nothing to force.

- [ ] **Step 5: Add the dispatch arms**

In the `case "$cmd"` block, beside the existing arms:

```bash
  build-console)
    assert_console_pins
    for s in "${console_services[@]}"; do build_console_one "$s"; done
    ;;
  all-consoles)
    if [ -n "$target" ]; then
      echo "usage: ci/images/run.sh all-consoles takes no service argument — use 'build-console [iam|gateway]' to build one" >&2
      exit 1
    fi
    assert_console_pins
    for s in "${console_services[@]}"; do build_console_one "$s"; done
    smoke_consoles
    ;;
```

Add `console_services` beside the existing `services` array assignment, scoped the same way by `$target`. Update the usage comment at the top of the file to list the two new commands.

`smoke_consoles` does not exist yet — Task 4 adds it. Until then `all-consoles` fails with `command not found`, which is correct and visible.

- [ ] **Step 6: Verify the build arm works end to end**

```bash
ci/images/run.sh build-console iam
docker images --format '{{.Repository}}:{{.Tag}}' | grep iam-console
```

Expected: `assert_console_pins` prints its green line, the build succeeds, and `iam-console:dev` exists.

- [ ] **Step 7: Commit**

```bash
git add ci/images/run.sh
git commit -m "feat(ci): build the console images from ci/images/run.sh (SMA-513)"
```

---

### Task 4: `smoke_consoles` — the assertion that catches a missing asset tree

**Files:**
- Modify: `ci/images/run.sh`
- Delete: `ts/tests/docker/serves-chunks.sh` (its logic moves into `run.sh`)

**Interfaces:**
- Consumes: `build_console_one`, `app_for`, `base_path_for` from Task 3; `assert_base_intact`'s shape as the model for the identity checks.
- Produces: `smoke_consoles()`, called by the `all-consoles` dispatch arm.

- [ ] **Step 1: Add `smoke_consoles`**

Model the container naming on the existing `RUN_ID`/`$$` convention, so two concurrent runs against one daemon never collide.

```bash
# Spec § 5.4. A text allowlist over COPY instructions cannot see a missing .next/static — the
# standalone tree has none of its own (SMA-510) and the staging copy is what supplies it. This
# asserts the served chunk instead, which is the only form that fails on the real defect.
smoke_consoles() {
  local service app base_path other name port origin html chunk bytes code uid
  trap 'for n in $CONSOLE_NAMES; do docker rm -f "$n" >/dev/null 2>&1 || true; done' RETURN
  CONSOLE_NAMES=""
  for service in iam gateway; do
    app="$(app_for "$service")"
    base_path="$(base_path_for "$service")"
    if [ "$service" = "iam" ]; then other="/gateway"; else other="/iam"; fi
    name="smoke-${app}-${RUN_ID}"
    CONSOLE_NAMES="$CONSOLE_NAMES $name"

    docker run -d --name "$name" -p 0:3000 \
      -e PAIGASUS_ZONE="$service" \
      -e PAIGASUS_ZONES='{"iam":"/iam","gateway":"/gateway"}' \
      "${app}:dev" >/dev/null
    port="$(docker port "$name" 3000/tcp)"; port="${port##*:}"
    origin="http://127.0.0.1:${port}"

    html="$(curl -fsS --retry 30 --retry-delay 1 --retry-all-errors "${origin}${base_path}/")"
    chunk="$(printf '%s' "$html" | grep -oE "${base_path}/_next/static/[^\"']+\.js" | sort -u | sed -n 1p)"
    if [ -z "$chunk" ]; then
      echo "::error::${app}: no ${base_path}/_next/static/*.js URL in the rendered page — .next/static was not staged into the image." >&2
      return 1
    fi
    bytes="$(curl -fsS "${origin}${chunk}" | wc -c | tr -d ' ')"
    if [ "${bytes:-0}" -lt 1 ]; then
      echo "::error::${app}: ${chunk} served an empty body — .next/static was not staged into the image." >&2
      return 1
    fi
    code="$(curl -s -o /dev/null -w '%{http_code}' "${origin}${other}${chunk#"$base_path"}")"
    if [ "$code" != "404" ]; then
      echo "::error::${app}: ${other} also served the chunk (HTTP ${code}); the two zones' asset prefixes collide." >&2
      return 1
    fi

    uid="$(docker top "$name" -o uid | sed -n 2p | tr -d ' ')"
    if [ "$uid" != "65532" ]; then
      echo "::error::${app} runs as uid ${uid}; the console images must run as 65532." >&2
      return 1
    fi
    if docker run --rm --entrypoint /bin/sh "${app}:dev" -c true >/dev/null 2>&1; then
      echo "::error::${app}:dev has a shell; the runtime base must stay distroless." >&2
      return 1
    fi
    echo "  ${app}: serves ${chunk} (${bytes} bytes), 404 under ${other}, uid 65532, no shell"
  done
  echo "== CONSOLE SMOKE OK =="
}
```

Note `sed -n 2p` and `sed -n 1p` rather than `head -1` or `awk '…exit'`: check 13 bans piping into an early-exit reader, and `sed -n Np` reads its whole input.

- [ ] **Step 2: Add the staged-tree parity assertion**

Spec § 5.5 assertion 4. This is what keeps the Dockerfile's staging and `moon.yml`'s staging in agreement, since PR 1 deliberately creates a second staging site.

Append inside the `for service` loop, after the identity checks:

```bash
    local host_dir img_list host_list
    host_dir="$ROOT/ts/apps/${app}/.next/standalone/apps/${app}/.next/static"
    if [ -d "$host_dir" ]; then
      img_list="$(docker run --rm --entrypoint /nodejs/bin/node "${app}:dev" \
        -e 'const {readdirSync}=require("fs");const w=(d,p="")=>readdirSync(d,{withFileTypes:true}).flatMap(e=>e.isDirectory()?w(d+"/"+e.name,p+e.name+"/"):[p+e.name]);console.log(w("/app/apps/'"${app}"'/.next/static").sort().join("\n"))')"
      host_list="$(cd "$host_dir" && find . -type f | sed 's#^\./##' | sort)"
      if [ "$img_list" != "$host_list" ]; then
        echo "::error::${app}: the image's staged .next/static differs from the host build's — ts/Dockerfile and ts/apps/${app}/moon.yml have drifted." >&2
        diff <(printf '%s\n' "$host_list") <(printf '%s\n' "$img_list") >&2 || true
        return 1
      fi
      echo "  ${app}: staged tree matches the host build"
    else
      echo "  ${app}: no host build present; staged-tree parity not checked this run"
    fi
```

The `else` arm is deliberate and must say so out loud rather than silently pass: in CI the host build may not have run, and a check that quietly skips is the failure mode this repository has paid for repeatedly.

- [ ] **Step 3: Prove the smoke fails on an unstaged image**

```bash
cp ts/Dockerfile /tmp/Dockerfile.sma513.bak
# Remove the staging RUN block by its first line's anchor.
python3 - <<'PY'
import re, pathlib
p = pathlib.Path("ts/Dockerfile")
s = p.read_text()
s = re.sub(r'RUN set -eu; \\\n(?:.*\\\n)*?    fi\n', '', s, count=1)
p.write_text(s)
PY
touch ts/Dockerfile
ci/images/run.sh build-console iam
bash -c 'set -e; source ci/images/run.sh >/dev/null 2>&1; smoke_consoles' || echo "red as expected"
```

Expected: red with `.next/static was not staged into the image`.

- [ ] **Step 4: Restore and prove it passes**

```bash
cp /tmp/Dockerfile.sma513.bak ts/Dockerfile && touch ts/Dockerfile
ci/images/run.sh all-consoles
```

Expected: both consoles print their green lines and `== CONSOLE SMOKE OK ==`.

- [ ] **Step 5: Remove the scaffold script**

```bash
git rm ts/tests/docker/serves-chunks.sh
```

Its logic now lives in `smoke_consoles`. Leaving both would create a second, unpinned copy of the assertion.

- [ ] **Step 6: Commit**

```bash
git add ci/images/run.sh
git commit -m "feat(ci): smoke the console images on a served chunk, not a probe (SMA-513)"
```

---

### Task 5: Dependabot coverage and the images workflow

**Files:**
- Modify: `.github/dependabot.yml`
- Modify: `.github/workflows/images.yml`

**Interfaces:**
- Consumes: the `all-consoles` dispatch arm from Tasks 3 and 4.

- [ ] **Step 1: Confirm the gap**

```bash
grep -n 'package-ecosystem: docker' -A2 .github/dependabot.yml
```

Expected: one block, `directory: /rs`. `ts/Dockerfile` is uncovered, so its digest would be pinned and never refreshed — the posture the comment at lines 113-116 says this repository rejected.

- [ ] **Step 2: Add a `/ts` docker block**

Insert after the `/rs` docker block, mirroring its schedule and its grouping. Add `ignore` entries for major, minor and patch on the distroless image, with the reason stated:

```yaml
  # SMA-513: ts/Dockerfile pins gcr.io/distroless/nodejs24-debian12 by digest. Covered here for
  # the same reason the /rs block exists — a pinned-and-never-updated base is the mirror image of
  # the floating-tag risk. The version-update ignores are deliberate: assert_console_pins in
  # ci/images/run.sh couples the base's Node MAJOR to .prototools' node pin, so an automated
  # major bump is a red Dependabot cannot fix on its own. Digest updates within a major still
  # flow through.
  - package-ecosystem: docker
    directory: /ts
    schedule:
      interval: weekly
      day: monday
      time: "06:00"
    ignore:
      - dependency-name: "gcr.io/distroless/nodejs24-debian12"
        update-types:
          - version-update:semver-major
```

- [ ] **Step 3: Validate the YAML parses**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 -c "import yaml,sys; d=yaml.safe_load(open('.github/dependabot.yml')); print([u['directory'] for u in d['updates'] if u['package-ecosystem']=='docker'])"
```

Expected: `['/rs', '/ts']`.

- [ ] **Step 4: Extend `images.yml`**

Add `ts/**` inputs to the `pull_request` path filter and `ci/images/**` is already there. Add a step running the console path. Write `branches:` and `paths:` as **block sequences**, never the inline `[main]` form — `repo:actionlint`'s extractor does not parse inline flow and fails all four keys loudly.

```yaml
      - name: Build + smoke both consoles
        run: ci/images/run.sh all-consoles
```

Add these to the `pull_request` `paths:` block:

```yaml
            - ts/Dockerfile
            - ts/.dockerignore
            - ts/pnpm-lock.yaml
```

Do **not** add a bare `ts/**`. The existing filter deliberately excludes bare `rs/**` on pull requests to avoid two cold release builds per PR; the same reasoning applies here, and a console build is not cheap.

- [ ] **Step 5: Run actionlint**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
actionlint .github/workflows/images.yml
```

Expected: no output.

Note the full `repo:actionlint` gate has no working local bash on this machine. Running `actionlint` on the one file directly is the available local check; CI runs the gate.

- [ ] **Step 6: Commit**

```bash
git add .github/dependabot.yml .github/workflows/images.yml
git commit -m "ci: build and smoke the console images in CI, and cover ts/Dockerfile with Dependabot (SMA-513)"
```

---

### Task 6: Rewrite RUNBOOK § 6 as-built

**Files:**
- Modify: `docs/ops/RUNBOOK-containers.md` (§ 6, currently prescriptive)

- [ ] **Step 1: Re-read the section as PR 270 left it**

```bash
sed -n '/^## 6\./,/^## 7\./p' docs/ops/RUNBOOK-containers.md
```

- [ ] **Step 2: Replace the future-tense conventions with what shipped**

Rewrite § 6 to describe the console images as built, keeping the same five bullets but stating the realized choice for each: the distroless base and its digest; uid 65532, matching the Rust images so one `securityContext` covers all four; the two fixed-path `.mjs` files and the `ENTRYPOINT`-cannot-expand-`ARG` reason for them; runtime-only configuration, with the `PAIGASUS_COMPILED_*` exception named; and the Dependabot `/ts` block.

Add a sixth bullet that did not exist before, because it is the trap that cost this issue a blocker:

> **The standalone output has no static assets.** Next writes no `.next/static` and no `public/`
> into `.next/standalone`. The console image's builder stages both, exactly as
> `ts/apps/<app>/moon.yml`'s `build` task does. An image built without that copy answers 200 on
> `<basePath>/healthz` and 404 on every chunk, so a probe-based smoke test cannot see it —
> `smoke_consoles` in `ci/images/run.sh` asserts a **served chunk** instead, and a staged-tree
> parity check keeps the two staging sites in agreement.

- [ ] **Step 3: Confirm the edit introduced no unmarked mention of the CI report artifact**

`ci/actionlint/run.sh` check 12 requires a `<!-- moon-diagnosis:ok -->` (or `:superseded`) marker on any file that names Moon's CI report JSON artifact, unless the file is in `CIREPORT_MENTIONS_ALLOWED`. Check the count without writing the literal into this plan — writing it here would put the same obligation on this file:

```bash
needle="$(printf 'ciReport%s.json' '')"
grep -c "$needle" docs/ops/RUNBOOK-containers.md || true
```

Expected: `0`, or a non-zero count on a file that already carries the marker. If the count rose because of this edit, either reword or add the marker.

- [ ] **Step 4: Commit**

```bash
git add docs/ops/RUNBOOK-containers.md
git commit -m "docs(repo): record the console image conventions as built (SMA-513)"
```

---

# PR 2a — The chart, the helm pin, and the golden files

### Task 7: Pin `helm` through proto

Spec § 6 and D8. Every other CLI gate in this repository is proto-pinned, and `helm template` output varies with the binary, so an unpinned helm makes the golden files red at random.

**Files:**
- Modify: `.prototools`
- Create: `.proto/plugins/helm.toml`

**Interfaces:**
- Produces: a `helm` resolvable at `$HOME/.proto/shims/helm`, used by Tasks 8–13.

- [ ] **Step 1: Read an existing plugin TOML as the model**

```bash
cat .proto/plugins/actionlint.toml
cat .proto/plugins/promtool.toml
```

Both are vendored schema plugins. Note the `[platform.*.arch]` override shape — the `.prototools` comment records that proto 0.61.1 is the floor for it, and that a global `{arch}` remap is global-only.

- [ ] **Step 2: Write `.proto/plugins/helm.toml`**

Model it on `promtool.toml`, which is the closest shape: a GitHub-release archive with per-platform naming. Helm's release assets are at `https://get.helm.sh/helm-v{version}-{os}-{arch}.tar.gz`, and the binary sits under `{os}-{arch}/helm` inside the archive.

- [ ] **Step 3: Add the pin and install it**

```bash
export PROTO_REPORTER=text
# Add `helm = "3.19.0"` to .prototools' tool block, and the plugin entry to [plugins].
proto install helm
proto bin helm --reporter text
```

Expected: a path. **If the output is JSON, the reporter override did not take** — `proto` emits NDJSON on stdout inside an agent session and that breaks any captured `$(proto …)`. Re-run with `PROTO_REPORTER=text` exported.

- [ ] **Step 4: Verify the version and record it**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
helm version --short
```

Expected: `v3.19.0+…`. Append an M2 section to `docs/superpowers/specs/2026-09-19-sma-513-measurements.md` recording the exact version, because every golden file in Task 13 is pinned against it.

- [ ] **Step 5: Commit**

```bash
git add .prototools .proto/plugins/helm.toml docs/superpowers/specs/2026-09-19-sma-513-measurements.md
git commit -m "build(deps): pin helm through proto for the chart gate (SMA-513)"
```

---

### Task 8: Chart skeleton, values, and the four refusals

Spec § 7.1 and § 7.7. The validation goes in one `paigasus.validate` include called from the top of every template, because Helm aborts on the first `fail` it reaches and template evaluation order is not a stable contract.

**Files:**
- Create: `charts/paigasus/Chart.yaml`
- Create: `charts/paigasus/values.yaml`
- Create: `charts/paigasus/templates/_helpers.tpl`
- Create: `charts/paigasus/tests/refusals.sh`

**Interfaces:**
- Produces: `paigasus.validate`, `paigasus.enabledZones` (a sorted list of enabled zone ids), `paigasus.zoneMapJson`, `paigasus.serviceMapJson`, and `paigasus.fullname`. Tasks 9–12 consume all five.

- [ ] **Step 1: Write the failing test first**

`charts/paigasus/tests/refusals.sh` asserts the four rows of spec § 7.7. It must fail now, because no chart exists.

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Spec § 7.7. Four values combinations, two valid and two refused. A refused one must fail with
# ITS OWN message, not with an incidental template error from somewhere else — otherwise the
# chart is refusing by accident and a later edit silently makes it install.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHART="$(cd "$HERE/.." && pwd)"
KUBE_VERSION="1.31.0"
ec=0

render() { helm template t "$CHART" --kube-version "$KUBE_VERSION" "$@" 2>&1; }

expect_fail() {
  local label="$1" needle="$2"; shift 2
  local out
  out="$(render "$@")" && { echo "FAIL [$label]: rendered, expected a refusal"; ec=1; return; }
  if grep -qF -- "$needle" < <(printf '%s' "$out"); then
    echo "  ok [$label]: refused with its own message"
  else
    echo "FAIL [$label]: refused, but not with \"$needle\""; printf '%s\n' "$out"; ec=1
  fi
}

expect_render() {
  local label="$1"; shift
  if render "$@" >/dev/null; then echo "  ok [$label]: renders"; else
    echo "FAIL [$label]: expected a successful render"; render "$@"; ec=1
  fi
}

expect_fail "no zone enabled" "at least one zone must be enabled" \
  --set zones.iam.enabled=false --set zones.gateway.enabled=false
expect_fail "gateway without iam" "the gateway zone requires the iam zone" \
  --set zones.iam.enabled=false --set zones.gateway.enabled=true
expect_fail "unknown zone id" "is not a known service slug" \
  --set zones.frobnicate.enabled=true --set zones.frobnicate.basePath=/frobnicate
expect_render "iam only" --set zones.gateway.enabled=false
expect_render "iam and gateway" --set zones.gateway.enabled=true

[ "$ec" -eq 0 ] && echo "== chart refusals OK =="
exit "$ec"
```

```bash
chmod +x charts/paigasus/tests/refusals.sh
```

- [ ] **Step 2: Run it and verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
charts/paigasus/tests/refusals.sh
```

Expected: FAIL on every row — the chart directory has no `Chart.yaml`.

- [ ] **Step 3: Write `Chart.yaml`**

```yaml
# SPDX-License-Identifier: Apache-2.0
apiVersion: v2
name: paigasus
description: Paigasus IAM and AI Gateway, with their console zones behind one origin
type: application
version: 0.1.0
appVersion: "0.0.0"
```

`appVersion` is `"0.0.0"` because `paigasus-gateway` and `paigasus-iam` are pinned there deliberately — their `env!("CARGO_PKG_VERSION")` feeds `ServiceInfo` and ADR-0020 skew reporting is parked on that value. SMA-658 ends that pin; when it does, this follows.

- [ ] **Step 4: Write `values.yaml`**

Use the exact ports from the Global Constraints table. Every key carries a comment saying what it feeds.

```yaml
# SPDX-License-Identifier: Apache-2.0
#
# ONE values block. Six projections derive from `zones` and from nothing else: the ingress path
# rules, PAIGASUS_ZONES, PAIGASUS_SERVICES, PAIGASUS_IAM_GRPC_URL, the console Deployments and the
# backend Deployments. A zone's `enabled` flag governs ALL of them together — that is what makes
# "routable but unadvertised" unrepresentable rather than merely checked (spec D6).
#
# A zone id MUST be a member of SERVICE_SLUGS (ts/packages/paigasus-discovery/src/core/state.ts).
# parseServiceMap THROWS on an unknown key, in BOTH consoles, at first request (spec F8).
zones:
  iam:
    enabled: true
    basePath: /iam
    console:
      image:
        repository: ghcr.io/smk1085/paigasus-iam-console
        tag: ""          # defaults to .Chart.AppVersion
      replicas: 2
    backend:
      image:
        repository: ghcr.io/smk1085/paigasus-iam
        tag: ""
      # replicas and maxSurge are pinned; see the comment in templates/backend-deployment.yaml.
      httpPort: 8080
      grpcPort: 9090
      migrationLockWaitSeconds: 120
  gateway:
    enabled: false
    basePath: /gateway
    console:
      image:
        repository: ghcr.io/smk1085/paigasus-gateway-console
        tag: ""
      replicas: 2
    backend:
      image:
        repository: ghcr.io/smk1085/paigasus-gateway
        tag: ""
      httpPort: 8088

ingress:
  host: ""               # REQUIRED. Feeds PAIGASUS_PUBLIC_ORIGIN as https://<host>.
  className: ""
  tlsSecretName: ""      # REQUIRED. The ingress must terminate TLS: PAIGASUS_PUBLIC_ORIGIN is
                         # validated as https, and __Host-pgs_sid requires Secure.
  annotations: {}
  # DO NOT add a rewrite annotation. Each console compiles basePath in and serves its full path
  # already; a rewrite that strips /iam breaks every route in that zone.

oidc:
  issuer: ""             # REQUIRED
  clientId: ""           # REQUIRED
  existingSecret: ""     # REQUIRED. Must hold keys: oidc-client-secret, session-redis-url.

redis: {}                # the URL lives in oidc.existingSecret, since it may carry a password

postgres:
  host: ""               # REQUIRED by the IAM backend
  existingSecret: ""     # REQUIRED
```

- [ ] **Step 5: Write `_helpers.tpl`**

```
{{/* SPDX-License-Identifier: Apache-2.0 */}}

{{/*
The closed service-slug registry. parseServiceMap validates every PAIGASUS_SERVICES key against
the TypeScript SERVICE_SLUGS and THROWS on an unknown one, in both consoles, at first request.
Keep this list equal to ts/packages/paigasus-discovery/src/core/state.ts.
*/}}
{{- define "paigasus.serviceSlugs" -}}
iam gateway
{{- end -}}

{{- define "paigasus.fullname" -}}
{{- printf "%s-%s" .Release.Name .Chart.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "paigasus.enabledZones" -}}
{{- $out := list -}}
{{- range $id, $z := .Values.zones -}}
{{- if $z.enabled -}}{{- $out = append $out $id -}}{{- end -}}
{{- end -}}
{{- join " " (sortAlpha $out) -}}
{{- end -}}

{{/*
Every refusal lives here, and every template calls this first. Helm aborts on the first `fail` it
reaches and evaluation order across files is not a stable contract, so a refusal placed in one
template would fire only when that template happens to render first.
*/}}
{{- define "paigasus.validate" -}}
{{- $enabled := splitList " " (include "paigasus.enabledZones" .) | compact -}}
{{- $slugs := splitList " " (include "paigasus.serviceSlugs" .) -}}
{{- range $id, $z := .Values.zones -}}
{{- if and $z.enabled (not (has $id $slugs)) -}}
{{- fail (printf "zones.%s: \"%s\" is not a known service slug (expected one of %s). parseServiceMap would throw in BOTH consoles at first request." $id $id (join ", " $slugs)) -}}
{{- end -}}
{{- end -}}
{{- if eq (len $enabled) 0 -}}
{{- fail "at least one zone must be enabled; a chart with no zone serves nothing" -}}
{{- end -}}
{{- if and (has "gateway" $enabled) (not (has "iam" $enabled)) -}}
{{- fail "the gateway zone requires the iam zone: gateway-console's PAIGASUS_SERVICES schema refuses to construct without an \"iam\" entry, so its pods would crash-loop" -}}
{{- end -}}
{{- if not .Values.ingress.host -}}
{{- fail "ingress.host is required; it is the single origin every zone's cookie is scoped to" -}}
{{- end -}}
{{- end -}}

{{- define "paigasus.zoneMapJson" -}}
{{- $m := dict -}}
{{- range $id, $z := .Values.zones -}}
{{- if $z.enabled -}}{{- $_ := set $m $id $z.basePath -}}{{- end -}}
{{- end -}}
{{- toJson $m -}}
{{- end -}}

{{- define "paigasus.serviceMapJson" -}}
{{- $m := dict -}}
{{- $full := include "paigasus.fullname" . -}}
{{- range $id, $z := .Values.zones -}}
{{- if $z.enabled -}}
{{- $_ := set $m $id (printf "http://%s-%s-backend:%d" $full $id (int $z.backend.httpPort)) -}}
{{- end -}}
{{- end -}}
{{- toJson $m -}}
{{- end -}}
```

- [ ] **Step 6: Add a trivial template so `helm template` has something to render**

`charts/paigasus/templates/validate.yaml`:

```yaml
{{/* SPDX-License-Identifier: Apache-2.0 */}}
{{- include "paigasus.validate" . -}}
```

- [ ] **Step 7: Run the refusal test and verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
charts/paigasus/tests/refusals.sh --set ingress.host=console.example.test
```

Expected: five `ok` lines and `== chart refusals OK ==`.

If `unknown zone id` does not fail: `--set zones.frobnicate.enabled=true` adds a key whose `basePath` and `backend` are unset, so a template may error on the missing `backend.httpPort` before `paigasus.validate` runs. Confirm the message is the slug one, not a nil-pointer one. That distinction is the whole point of the row.

- [ ] **Step 8: Commit**

```bash
git add charts/paigasus
git commit -m "feat(repo): chart skeleton with the single zones values block and its four refusals (SMA-513)"
```

---

### Task 9: The two ConfigMaps and the Secret reference

Spec § 7.4. The Secret needs its own checksum annotation for a concrete reason: with `envFrom` and no checksum, rotating `PAIGASUS_SESSION_REDIS_URL` restarts nothing and every pod keeps the old value indefinitely.

**Files:**
- Create: `charts/paigasus/templates/zonemap-configmap.yaml`
- Create: `charts/paigasus/templates/console-env-configmap.yaml`
- Modify: `charts/paigasus/tests/refusals.sh` (add an assertion file for maps)
- Create: `charts/paigasus/tests/maps.sh`

**Interfaces:**
- Consumes: `paigasus.fullname`, `paigasus.zoneMapJson`, `paigasus.serviceMapJson`, `paigasus.validate`.
- Produces: ConfigMaps named `<fullname>-zonemap` and `<fullname>-console-env`. Task 10 mounts both with `envFrom` and annotates on their checksums.

- [ ] **Step 1: Write the failing test**

`charts/paigasus/tests/maps.sh`:

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# The zone map and the service map must carry EXACTLY the enabled zones — no more, no less. The
# "no more" half is its own assertion because the key-set comparison in the gate compares sets
# that all derive from one `range`, so it cannot catch a value written outside that range.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHART="$(cd "$HERE/.." && pwd)"
BASE=(--kube-version 1.31.0 --set ingress.host=console.example.test)
ec=0

keys() {  # keys() <json>
  printf '%s' "$1" | python3 -c 'import json,sys;print(" ".join(sorted(json.load(sys.stdin))))'
}

check() {
  local label="$1" want="$2"; shift 2
  local out zones services
  out="$(helm template t "$CHART" "${BASE[@]}" "$@")"
  zones="$(printf '%s' "$out" | python3 -c '
import sys,yaml
for d in yaml.safe_load_all(sys.stdin):
    if d and d.get("kind")=="ConfigMap" and d["metadata"]["name"].endswith("-zonemap"):
        print(d["data"]["PAIGASUS_ZONES"]); break')"
  services="$(printf '%s' "$out" | python3 -c '
import sys,yaml
for d in yaml.safe_load_all(sys.stdin):
    if d and d.get("kind")=="ConfigMap" and d["metadata"]["name"].endswith("-zonemap"):
        print(d["data"]["PAIGASUS_SERVICES"]); break')"
  for pair in "zones:$zones" "services:$services"; do
    local name="${pair%%:*}" json="${pair#*:}" got
    got="$(keys "$json")"
    if [ "$got" != "$want" ]; then
      echo "FAIL [$label/$name]: got \"$got\", want \"$want\""; ec=1
    else
      echo "  ok [$label/$name]: $got"
    fi
  done
}

check "iam only"        "iam"         --set zones.gateway.enabled=false
check "iam and gateway" "gateway iam" --set zones.gateway.enabled=true

[ "$ec" -eq 0 ] && echo "== chart maps OK =="
exit "$ec"
```

```bash
chmod +x charts/paigasus/tests/maps.sh
```

- [ ] **Step 2: Run it and verify it fails**

```bash
charts/paigasus/tests/maps.sh
```

Expected: FAIL — no ConfigMap exists, so the extraction yields an empty string.

- [ ] **Step 3: Write `zonemap-configmap.yaml`**

```yaml
{{/* SPDX-License-Identifier: Apache-2.0 */}}
{{- include "paigasus.validate" . -}}
apiVersion: v1
kind: ConfigMap
metadata:
  name: {{ include "paigasus.fullname" . }}-zonemap
data:
  # Byte-identical across every console pod BY CONSTRUCTION — one object, mounted by both — rather
  # than by template discipline. assertCompiledAgreement() throws on a pod whose OWN entry
  # disagrees, but it does not check the other zones' entries, so this is the control, not it.
  PAIGASUS_ZONES: {{ include "paigasus.zoneMapJson" . | quote }}
  PAIGASUS_SERVICES: {{ include "paigasus.serviceMapJson" . | quote }}
```

- [ ] **Step 4: Write `console-env-configmap.yaml`**

```yaml
{{/* SPDX-License-Identifier: Apache-2.0 */}}
{{- include "paigasus.validate" . -}}
{{- $full := include "paigasus.fullname" . -}}
apiVersion: v1
kind: ConfigMap
metadata:
  name: {{ $full }}-console-env
data:
  PAIGASUS_PUBLIC_ORIGIN: {{ printf "https://%s" .Values.ingress.host | quote }}
  PAIGASUS_OIDC_ISSUER: {{ .Values.oidc.issuer | quote }}
  PAIGASUS_OIDC_CLIENT_ID: {{ .Values.oidc.clientId | quote }}
  PAIGASUS_IAM_GRPC_URL: {{ printf "http://%s-iam-backend:%d" $full (int .Values.zones.iam.backend.grpcPort) | quote }}
  # Forced, not exposed. createAuthRuntime refuses "memory" once PAIGASUS_ZONES names more than
  # one zone, and console-core's descriptor cache needs Redis regardless. A knob whose only other
  # setting crash-loops the pod is not a knob.
  PAIGASUS_SESSION_STORE: "redis"
```

- [ ] **Step 5: Run the test and verify it passes**

```bash
charts/paigasus/tests/maps.sh
```

Expected: four `ok` lines and `== chart maps OK ==`.

- [ ] **Step 6: Prove the "no more" half bites**

Temporarily add a literal orphan key to `zonemap-configmap.yaml`'s `PAIGASUS_SERVICES` — render it as `{{ include "paigasus.serviceMapJson" . | ... }}` replaced by a hardcoded `'{"iam":"http://x","gateway":"http://y"}'` — and re-run with `--set zones.gateway.enabled=false`.

Expected: `FAIL [iam only/services]: got "gateway iam", want "iam"`.

Restore by deleting the inserted line, not with `git checkout --`.

- [ ] **Step 7: Commit**

```bash
git add charts/paigasus
git commit -m "feat(repo): render the zone and service maps from one shared ConfigMap (SMA-513)"
```

---

### Task 10: Console Deployment and Service

**Files:**
- Create: `charts/paigasus/templates/console-deployment.yaml`
- Create: `charts/paigasus/templates/console-service.yaml`

**Interfaces:**
- Consumes: both ConfigMaps from Task 9, `oidc.existingSecret`.
- Produces: one Deployment and one Service per enabled zone, named `<fullname>-<id>-console`.

- [ ] **Step 1: Write `console-deployment.yaml`**

```yaml
{{/* SPDX-License-Identifier: Apache-2.0 */}}
{{- include "paigasus.validate" . -}}
{{- $root := . -}}
{{- $full := include "paigasus.fullname" . -}}
{{- range $id, $z := .Values.zones }}
{{- if $z.enabled }}
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: {{ $full }}-{{ $id }}-console
spec:
  replicas: {{ $z.console.replicas }}
  selector:
    matchLabels:
      app.kubernetes.io/name: {{ $id }}-console
      app.kubernetes.io/instance: {{ $root.Release.Name }}
  template:
    metadata:
      labels:
        app.kubernetes.io/name: {{ $id }}-console
        app.kubernetes.io/instance: {{ $root.Release.Name }}
      annotations:
        # All three checksums, not two. With envFrom and no checksum on the SECRET, rotating
        # PAIGASUS_SESSION_REDIS_URL or the OIDC client secret restarts nothing and every pod
        # keeps the old value indefinitely.
        checksum/zonemap: {{ include (print $root.Template.BasePath "/zonemap-configmap.yaml") $root | sha256sum }}
        checksum/console-env: {{ include (print $root.Template.BasePath "/console-env-configmap.yaml") $root | sha256sum }}
        checksum/secret: {{ $root.Values.oidc.existingSecret | sha256sum }}
    spec:
      securityContext:
        runAsUser: 65532
        runAsGroup: 65532
        runAsNonRoot: true
        # NO readOnlyRootFilesystem. RUNBOOK-containers.md § 7: that posture is untested for
        # every image in this repo. It is a follow-up to measure, not a default to ship.
      containers:
        - name: console
          image: "{{ $z.console.image.repository }}:{{ $z.console.image.tag | default $root.Chart.AppVersion }}"
          ports:
            - name: http
              containerPort: 3000
          env:
            - name: PAIGASUS_ZONE
              value: {{ $id | quote }}
            - name: PORT
              value: "3000"
          envFrom:
            - configMapRef:
                name: {{ $full }}-zonemap
            - configMapRef:
                name: {{ $full }}-console-env
            - secretRef:
                name: {{ $root.Values.oidc.existingSecret }}
          # Liveness is a TCP check, NOT httpGet on /healthz. That route runs the FULL config
          # parse and its failure is deliberately not memoized, so using it for liveness puts a
          # misconfigured pod into CrashLoopBackOff instead of leaving it running and NotReady
          # with a readable log — and RUNBOOK-containers.md § 4 requires liveness never to touch
          # a dependency. The consoles have no /readyz; that is a recorded gap (spec § 7.8).
          livenessProbe:
            tcpSocket:
              port: http
            periodSeconds: 20
          readinessProbe:
            httpGet:
              path: {{ $z.basePath }}/healthz
              port: http
            periodSeconds: 10
            failureThreshold: 3
{{- end }}
{{- end }}
```

- [ ] **Step 2: Write `console-service.yaml`**

Same `range` shape, `ClusterIP`, port 3000 named `http`, selector matching the Deployment's labels.

- [ ] **Step 3: Verify both zones render and the per-zone values differ**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
helm template t charts/paigasus --kube-version 1.31.0 \
  --set ingress.host=console.example.test --set zones.gateway.enabled=true \
  | python3 -c '
import sys,yaml
for d in yaml.safe_load_all(sys.stdin):
    if d and d.get("kind")=="Deployment" and "console" in d["metadata"]["name"]:
        c=d["spec"]["template"]["spec"]["containers"][0]
        print(d["metadata"]["name"], c["image"],
              [e for e in c["env"] if e["name"]=="PAIGASUS_ZONE"],
              c["readinessProbe"]["httpGet"]["path"])'
```

Expected: two rows, each with its own image, its own `PAIGASUS_ZONE` and its own probe path.

- [ ] **Step 4: Commit**

```bash
git add charts/paigasus
git commit -m "feat(repo): console Deployment and Service per enabled zone (SMA-513)"
```

---

### Task 11: Backend Deployments and Services

Spec § 7.9. The IAM Deployment carries the SMA-559 rules, and `maxSurge: 0` is a **deviation** from the runbook, not its instruction — the comment must say so.

**Files:**
- Create: `charts/paigasus/templates/backend-deployment.yaml`
- Create: `charts/paigasus/templates/backend-service.yaml`

**Interfaces:**
- Consumes: `paigasus.fullname`, `postgres.*`, `oidc.existingSecret`.
- Produces: `<fullname>-<id>-backend` Deployment and Service. Task 9's `paigasus.serviceMapJson` already assumes exactly that Service name and `backend.httpPort`.

- [ ] **Step 1: Write `backend-deployment.yaml`**

Range over enabled zones as in Task 10. The IAM arm additionally carries:

```yaml
  replicas: 1
  strategy:
    type: RollingUpdate
    rollingUpdate:
      maxSurge: 0
      maxUnavailable: 1
```

with this comment directly above it:

```yaml
      # DELIBERATE DEVIATION, not the runbook's instruction. RUNBOOK-containers.md § 5 says
      # maxSurge need no longer be pinned to 0 now that the boot migration takes an advisory lock
      # (SMA-559). It is kept at 0 because that relaxation carries an unmet precondition:
      # AppState::new's reconcile_starter writes system policies and roles on EVERY boot with no
      # lock of its own and has never been tested under concurrency. That is a Rust concurrency
      # question needing a concurrency test, not a chart default, and it is filed separately.
      # Raise this only after that issue closes.
```

Probes, per RUNBOOK § 5:

```yaml
          startupProbe:          # sized for config load + Database::connect ONLY. SMA-571 removed
            httpGet:             # the coupling to IAM_MIGRATION__LOCK_WAIT_SECS entirely.
              path: /healthz
              port: http
            periodSeconds: 5
            failureThreshold: 12
          livenessProbe:
            httpGet:
              path: /healthz
              port: http
            periodSeconds: 20
          readinessProbe:        # /readyz answers 503 {"status":"migrating"} for as long as the
            httpGet:             # migration takes. A never-ready replica is simply absent from
              path: /readyz      # the endpoint list; this threshold does NOT bound that window —
              port: http         # size it for a steady-state database blip.
            periodSeconds: 10
            failureThreshold: 3
```

and the env var the runbook asks to stay exposed:

```yaml
            - name: IAM_MIGRATION__LOCK_WAIT_SECS
              value: {{ $z.backend.migrationLockWaitSeconds | quote }}
```

The gateway arm keeps `readinessProbe.periodSeconds: 30` with this comment:

```yaml
          # 30s floor, not a copy-paste slip. The gateway's /readyz issues a REAL gRPC introspect
          # call to IAM on every poll, and both its health routes sit inside its metrics and
          # correlation layers (SMA-504 D10), so probe traffic is metered and load-bearing.
```

Both arms carry the same `securityContext` as Task 10 and the same `terminationGracePeriodSeconds` note:

```yaml
      # 45s, not the 30s default. IAM drains a JoinSet of relays and maintainers on SIGTERM and
      # no worst-case drain time has ever been measured — treat 30s as a floor to widen from, and
      # revisit if a rolling update is observed truncating a drain.
      terminationGracePeriodSeconds: 45
```

- [ ] **Step 2: Write `backend-service.yaml`**

IAM exposes two ports, `http` and `grpc`; the gateway exposes `http` only. The Service name must be exactly `<fullname>-<id>-backend`, because `paigasus.serviceMapJson` builds its URL from that shape.

- [ ] **Step 3: Verify the service map resolves to a real Service**

```bash
helm template t charts/paigasus --kube-version 1.31.0 \
  --set ingress.host=console.example.test --set zones.gateway.enabled=true \
  | python3 -c '
import sys,yaml,json
docs=[d for d in yaml.safe_load_all(sys.stdin) if d]
svc={d["metadata"]["name"] for d in docs if d["kind"]=="Service"}
cm=[d for d in docs if d["kind"]=="ConfigMap" and d["metadata"]["name"].endswith("-zonemap")][0]
for k,v in json.loads(cm["data"]["PAIGASUS_SERVICES"]).items():
    host=v.split("//")[1].split(":")[0]
    print(k, host, "OK" if host in svc else "DANGLING")'
```

Expected: both rows `OK`. A `DANGLING` row means `paigasus.serviceMapJson` and `backend-service.yaml` disagree on the naming, which would render a permanently `degraded` tile — the exact state AC 2 must distinguish from `absent`.

- [ ] **Step 4: Commit**

```bash
git add charts/paigasus
git commit -m "feat(repo): backend Deployments and Services, with the SMA-559 rollout rules (SMA-513)"
```

---

### Task 12: The Ingress

**Files:**
- Create: `charts/paigasus/templates/ingress.yaml`
- Create: `charts/paigasus/tests/ingress.sh`

**Interfaces:**
- Consumes: `paigasus.fullname`, `ingress.*`, the console Services from Task 10.
- Produces: one Ingress with one rule per enabled zone.

- [ ] **Step 1: Write the failing test**

`charts/paigasus/tests/ingress.sh` asserts the three-way coupling of spec F2 — the ingress path set, the `PAIGASUS_ZONES` key set and the `PAIGASUS_SERVICES` key set agree, for both valid subsets — **and** that a disabled zone leaves no trace anywhere in the rendered output.

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHART="$(cd "$HERE/.." && pwd)"
ec=0

coupling() {
  local label="$1" want="$2"; shift 2
  local out got
  out="$(helm template t "$CHART" --kube-version 1.31.0 --set ingress.host=console.example.test "$@")"
  got="$(printf '%s' "$out" | python3 -c '
import sys,yaml,json
docs=[d for d in yaml.safe_load_all(sys.stdin) if d]
ing=[d for d in docs if d["kind"]=="Ingress"]
paths=sorted(p["path"] for i in ing for r in i["spec"]["rules"] for p in r["http"]["paths"])
cm=[d for d in docs if d["kind"]=="ConfigMap" and d["metadata"]["name"].endswith("-zonemap")][0]
zones=json.loads(cm["data"]["PAIGASUS_ZONES"]); services=json.loads(cm["data"]["PAIGASUS_SERVICES"])
print("paths=%s zones=%s services=%s" % (",".join(paths), ",".join(sorted(zones)), ",".join(sorted(services))))
print("COUPLED" if sorted(zones)==sorted(services) and paths==sorted(zones.values()) else "DRIFT")')"
  if printf '%s' "$got" | grep -qF -- "DRIFT"; then
    echo "FAIL [$label]: $got"; ec=1
  else
    echo "  ok [$label]: $(printf '%s' "$got" | sed -n 1p)"
  fi
  # A disabled zone must leave NO trace — a separate assertion, because the comparison above
  # compares sets that all derive from one `range` and cannot see a value written outside it.
  local absent
  for absent in $want; do
    if printf '%s' "$out" | grep -qF -- "$absent"; then
      echo "FAIL [$label]: disabled zone \"$absent\" appears in the rendered output"; ec=1
    fi
  done
}

coupling "iam only" "gateway" --set zones.gateway.enabled=false
coupling "iam and gateway" "" --set zones.gateway.enabled=true

[ "$ec" -eq 0 ] && echo "== chart ingress coupling OK =="
exit "$ec"
```

```bash
chmod +x charts/paigasus/tests/ingress.sh
charts/paigasus/tests/ingress.sh
```

Expected: FAIL — no Ingress renders yet.

- [ ] **Step 2: Write `ingress.yaml`**

```yaml
{{/* SPDX-License-Identifier: Apache-2.0 */}}
{{- include "paigasus.validate" . -}}
{{- $root := . -}}
{{- $full := include "paigasus.fullname" . -}}
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: {{ $full }}
  {{- with .Values.ingress.annotations }}
  annotations:
    {{- toYaml . | nindent 4 }}
  {{- end }}
  # DO NOT add a rewrite annotation here or in values. Each console compiles basePath in and
  # serves its FULL path already — a rewrite that strips /iam breaks every route in that zone.
  # This is the change an operator is most likely to add by reflex.
spec:
  {{- with .Values.ingress.className }}
  ingressClassName: {{ . }}
  {{- end }}
  tls:
    # The ingress MUST terminate TLS: PAIGASUS_PUBLIC_ORIGIN is validated as https, and the
    # __Host-pgs_sid cookie requires Secure. One host, so the cookie is host-only and every zone
    # reads it (ADR-0017 decision 2).
    - hosts:
        - {{ .Values.ingress.host | quote }}
      secretName: {{ .Values.ingress.tlsSecretName }}
  rules:
    - host: {{ .Values.ingress.host | quote }}
      http:
        paths:
          {{- range $id, $z := .Values.zones }}
          {{- if $z.enabled }}
          # /iam/auth/* and /gateway/auth/* are IN-APP route handlers and fall under their zone's
          # own prefix, so they need no rule of their own.
          - path: {{ $z.basePath }}
            pathType: Prefix
            backend:
              service:
                name: {{ $full }}-{{ $id }}-console
                port:
                  name: http
          {{- end }}
          {{- end }}
```

- [ ] **Step 3: Run the test and verify it passes**

```bash
charts/paigasus/tests/ingress.sh
```

Expected: two `ok` lines and `== chart ingress coupling OK ==`.

- [ ] **Step 4: Prove the coupling assertion bites**

Break the coupling deliberately and confirm each half reports red.

```bash
# (a) an ingress rule written outside the range
python3 - <<'PY'
import pathlib
p = pathlib.Path("charts/paigasus/templates/ingress.yaml")
s = p.read_text()
s = s.replace("          {{- end }}\n          {{- end }}\n",
  "          {{- end }}\n          {{- end }}\n"
  "          - path: /gateway\n            pathType: Prefix\n"
  "            backend:\n              service:\n"
  "                name: {{ $full }}-gateway-console\n"
  "                port:\n                  name: http\n", 1)
p.write_text(s)
PY
charts/paigasus/tests/ingress.sh || echo "red as expected (a)"
git checkout -- charts/paigasus/templates/ingress.yaml
```

Expected: `FAIL [iam only]` on both the `DRIFT` row and the `disabled zone "gateway" appears` row.

`git checkout --` is safe here and only here: `ingress.yaml` was committed in Step 3's predecessor and holds no uncommitted work. Confirm with `git status --short charts/` first.

- [ ] **Step 5: Commit**

```bash
git add charts/paigasus
git commit -m "feat(repo): path-route one origin to both console zones (SMA-513)"
```

---

### Task 13: `helm lint`, the golden files, and the render helper

**Files:**
- Create: `charts/paigasus/tests/render.sh`
- Create: `charts/paigasus/tests/golden/iam-only.yaml`
- Create: `charts/paigasus/tests/golden/iam-and-gateway.yaml`
- Create: `charts/paigasus/README.md`

**Interfaces:**
- Produces: `charts/paigasus/tests/render.sh [--update]`, which PR 2b's `ci/helm-render/run.sh` calls rather than reimplementing.

- [ ] **Step 1: Write `render.sh`**

It renders both valid subsets with a **pinned `--kube-version`**, so the goldens do not move with the binary's default, and diffs against the committed files. `--update` re-baselines.

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Golden render of the two valid subsets. --kube-version is pinned because helm's default moves
# with the binary, and these files are a byte pin. Re-baseline deliberately with --update; a
# golden change is a reviewable event, never a mechanical edit to clear a red.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHART="$(cd "$HERE/.." && pwd)"
KUBE_VERSION="1.31.0"
UPDATE=0
[ "${1:-}" = "--update" ] && UPDATE=1
ec=0

FIXED=(
  --kube-version "$KUBE_VERSION"
  --set ingress.host=console.example.test
  --set ingress.tlsSecretName=console-tls
  --set oidc.issuer=https://idp.example.test/realms/paigasus
  --set oidc.clientId=paigasus-console
  --set oidc.existingSecret=paigasus-console-secret
  --set postgres.host=postgres.example.test
  --set postgres.existingSecret=paigasus-postgres-secret
)

render_one() {
  local name="$1"; shift
  local want="$HERE/golden/${name}.yaml" got
  got="$(helm template paigasus "$CHART" "${FIXED[@]}" "$@")"
  if [ "$UPDATE" -eq 1 ]; then
    mkdir -p "$HERE/golden"; printf '%s\n' "$got" > "$want"
    echo "  updated ${name}.yaml"; return
  fi
  if [ ! -f "$want" ]; then
    echo "FAIL [$name]: no golden file at $want — run render.sh --update"; ec=1; return
  fi
  if ! diff -u "$want" <(printf '%s\n' "$got"); then
    echo "FAIL [$name]: rendered output differs from the golden file"; ec=1
  else
    echo "  ok [$name]"
  fi
}

helm lint "$CHART" "${FIXED[@]}" >/dev/null || { echo "FAIL: helm lint"; helm lint "$CHART" "${FIXED[@]}"; ec=1; }
render_one "iam-only"        --set zones.gateway.enabled=false
render_one "iam-and-gateway" --set zones.gateway.enabled=true

[ "$ec" -eq 0 ] && echo "== chart render OK =="
exit "$ec"
```

```bash
chmod +x charts/paigasus/tests/render.sh
```

- [ ] **Step 2: Run it and verify it fails for the right reason**

```bash
charts/paigasus/tests/render.sh
```

Expected: `FAIL [iam-only]: no golden file` and the same for `iam-and-gateway`. `helm lint` must already pass — if it does not, fix the chart before baselining.

- [ ] **Step 3: Baseline the goldens and read them**

```bash
charts/paigasus/tests/render.sh --update
charts/paigasus/tests/render.sh
wc -l charts/paigasus/tests/golden/*.yaml
```

Expected: `ok` on both and `== chart render OK ==`.

**Read both golden files before committing.** They are the reviewable artifact of this whole PR. Check specifically: the two `checksum/*` annotations differ between the two files; `iam-only.yaml` contains no occurrence of the string `gateway`; and both Deployments carry `runAsUser: 65532` and no `readOnlyRootFilesystem`.

- [ ] **Step 4: Verify the goldens move when the chart does**

```bash
sed -i.bak 's/runAsUser: 65532/runAsUser: 1000/' charts/paigasus/templates/console-deployment.yaml
charts/paigasus/tests/render.sh || echo "red as expected"
mv charts/paigasus/templates/console-deployment.yaml.bak charts/paigasus/templates/console-deployment.yaml
charts/paigasus/tests/render.sh
```

Expected: a diff naming `runAsUser`, then green again.

- [ ] **Step 5: Write `charts/paigasus/README.md`**

Cover: the one values block and its six projections; the four refusals and why each exists; the no-rewrite rule; that a zone id must be a `SERVICE_SLUGS` member; that D6 makes "backend deployed, console absent" inexpressible, with the follow-up named; that the goldens are pinned to the helm version recorded in the measurements file; and that `render.sh --update` is a deliberate act.

- [ ] **Step 6: Run every chart test together**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
for t in refusals maps ingress render; do
  echo "== $t =="
  charts/paigasus/tests/$t.sh --set ingress.host=console.example.test 2>/dev/null \
    || charts/paigasus/tests/$t.sh
done
```

Expected: four `OK` banners. (`refusals.sh` takes the host flag; the others set it themselves. If a script rejects the argument, run it bare — the fallback in the loop covers that.)

- [ ] **Step 7: Commit**

```bash
git add charts/paigasus
git commit -m "feat(repo): golden render and helm lint for the paigasus chart (SMA-513)"
```

---

## Before opening either PR

- [ ] Run the full graph the way CI does. Per-project tasks do not run the repo gates.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :test-e2e \
  --base origin/main \
  --include-relations
```

- [ ] `repo:input-liveness` will fail if any glob this work declares matches zero tracked files. A new `charts/**` input must have tracked files behind it before the gate runs.
- [ ] If `repo:actionlint` reds, read it against the known local-bash split before treating it as a finding: it has **no working local bash** on this machine, and an rc 2 with a `pipe … holds only 512 bytes` message is a host condition, not a defect.
- [ ] If a gate's stdout is empty and stderr holds one line saying `declare: -A: invalid option` or `mapfile: command not found`, that is `/bin/bash` 3.2.57 running a bash-4+ gate, not a finding. Re-run it with `/opt/homebrew/bin/bash`.

---

## Self-review

**Spec coverage.** § 5.1 → Task 2 Step 3, Task 5. § 5.2 → Task 2 Step 3. § 5.3 → Task 1, Task 2 Steps 3 and 6. § 5.4 → Task 2 Step 2, Task 4 Step 1. § 5.5 → Task 3 Step 2 (assertions 1–3), Task 4 Step 2 (assertion 4), Task 4 Step 1 (assertion 5). § 6 → Task 7. § 7.1 → Task 8 Step 4. § 7.2 → **not covered here**; the port pin against the Rust `config.rs` literals is a gate check and belongs to PR 2b. § 7.3 → Task 8 Step 5 (`paigasus.validate`) and the values comment. § 7.4 → Task 9, Task 10 Step 1. § 7.5 → Task 9 Step 4. § 7.6 → Task 12. § 7.7 → Task 8 Steps 1 and 5. § 7.8 → Task 10 Step 1. § 7.9 → Task 11. § 8 and § 9 → PR 2b and PR 3, out of this plan by design. § 10 → Task 6, Task 13 Step 5.

**One gap accepted deliberately:** spec § 7.2's port pin has no task here. It reads two Rust files and belongs with the gate, not the chart. It is the first task of the PR 2b plan.

**Type consistency.** `paigasus.fullname`, `paigasus.enabledZones`, `paigasus.zoneMapJson`, `paigasus.serviceMapJson`, `paigasus.validate` and `paigasus.serviceSlugs` are defined in Task 8 Step 5 and used under those exact names in Tasks 9–13. The Service name shape `<fullname>-<id>-backend` is produced by Task 11 Step 2 and consumed by Task 9's `paigasus.serviceMapJson`; Task 11 Step 3 asserts they agree. `app_for`, `base_path_for`, `build_console_one`, `assert_console_pins` and `smoke_consoles` are defined in Tasks 3 and 4 and referenced under those names in the dispatch arms.

**Placeholder scan.** No TBD, no "add appropriate error handling", no "similar to Task N". Every code step carries its content.
