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

# Fix round 1 (SMA-513): a FULL, schema-valid dummy runtime config, not just PAIGASUS_ZONE and
# PAIGASUS_ZONES. Both consoles validate their ENTIRE runtime config (@paigasus/auth's OIDC/session
# shape, @paigasus/discovery's PAIGASUS_SERVICES, and PAIGASUS_IAM_GRPC_URL — see
# ts/apps/<app>/lib/config.ts) on the FIRST request, and a missing or malformed variable throws
# "Invalid runtime configuration", which the app renders as a 500. That 500 looks exactly like a
# missing .next/static from the outside — both leave the page with no chunk URL to find — so
# without this block the assertion cannot tell "not staged" from "not configured" apart.
# One shared array covers both zones: iam-console only requires a "iam" entry in PAIGASUS_SERVICES,
# gateway-console requires both "iam" AND "gateway" (ts/apps/gateway-console/lib/config.ts), so the
# map below carries both unconditionally. This block is deliberately lifted verbatim by Task 4 into
# ci/images/run.sh's smoke_consoles.
# WARNING, INTENTIONAL COUPLING: a new required key added to a console's `lib/config.ts` schema
# will make this array stale and turn every 200 here into a 500 — that is supposed to fail LOUDLY,
# not be silently worked around by adding a fallback or relaxing the assertion below.
runtime_env=(
  -e "PAIGASUS_OIDC_ISSUER=https://idp.example.com"
  -e "PAIGASUS_OIDC_CLIENT_ID=dummy-client"
  -e "PAIGASUS_OIDC_CLIENT_SECRET=dummy-secret"
  -e "PAIGASUS_PUBLIC_ORIGIN=https://console.example.com"
  -e "PAIGASUS_SESSION_STORE=memory"
  -e "PAIGASUS_SERVICES={\"iam\":\"http://iam:8080\",\"gateway\":\"http://gateway:8080\"}"
  -e "PAIGASUS_IAM_GRPC_URL=http://iam:9090"
)

docker run -d --name "$name" -p 0:3000 \
  -e PAIGASUS_ZONE="${base_path#/}" \
  -e PAIGASUS_ZONES="{\"iam\":\"/iam\",\"gateway\":\"/gateway\"}" \
  "${runtime_env[@]}" \
  "$image" >/dev/null
port="$(docker port "$name" 3000/tcp)"; port="${port##*:}"
origin="http://127.0.0.1:${port}"

# Step 1: the page renders. Fetch the CANONICAL url (no trailing slash) — Next compiles an
# internal, priority 308 redirect for "<basePath>/" -> "<basePath>" whenever trailingSlash is
# false (the default; neither console's next.config.ts sets it), confirmed by reading
# .next/routes-manifest.json out of the built image. `curl -fsS` alone does not follow a
# redirect, so a request to the trailing-slash form "succeeds" with the tiny redirect body (e.g.
# 4 bytes: "/iam") instead of the real page — indistinguishable, from this script's point of
# view, from a page that failed to render. `-L --max-redirs 3` is kept even though the canonical
# URL does not redirect today, so this keeps working if trailingSlash is ever flipped to true and
# the redirect direction reverses.
html="$(curl -fsSL --max-redirs 3 --retry 20 --retry-delay 1 --retry-all-errors "${origin}${base_path}")"

# Step 2: extract one chunk URL. No pipe into an early-exit reader: capture, then read.
# `grep -oE` exits 1 on no match even after reading its whole input; under set -euo pipefail an
# unguarded assignment from that pipeline would exit the script here instead of reaching the
# named failure branch below, so the trailing `|| true` hands the empty-string case to the `-z`
# check instead.
chunk="$(printf '%s' "$html" | grep -oE "${base_path}/_next/static/[^\"']+\.js" | sort -u | sed -n 1p)" || true
if [ -z "$chunk" ]; then
  echo "::error::${image}: no ${base_path}/_next/static/*.js URL in the rendered page" \
    "— either .next/static was not staged into the image, or the page did not render at all" \
    "(an unfollowed redirect, or invalid runtime configuration)" >&2
  exit 1
fi

# Step 3: the chunk is served, with a body. When .next/static was never staged, the file is
# simply ABSENT from the served tree, so this is a straight 404, not a 200 with an empty body —
# `curl -fsS` fails outright. Guarded the same way as Step 2's `grep -oE`: without the trailing
# `|| true`, `set -e` would abort the script right here on that 404, before the informative
# message below ever runs.
bytes="$(curl -fsS "${origin}${chunk}" | wc -c | tr -d ' ')" || true
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
