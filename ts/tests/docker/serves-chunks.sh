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

# Step 1: the page renders. Two curl calls, not one.
#
# First, a status-only probe. Fetch the CANONICAL url (no trailing slash) — Next compiles an
# internal, priority 308 redirect for "<basePath>/" -> "<basePath>" whenever trailingSlash is
# false (the default; neither console's next.config.ts sets it), confirmed by reading
# .next/routes-manifest.json out of the built image. `-L --max-redirs 3` follows that redirect if
# it ever fires (kept even though the canonical URL does not redirect today, so this keeps working
# if trailingSlash is ever flipped to true and the redirect direction reverses); `--max-time`
# bounds the request so a hung connection, or `-L` chasing an off-origin redirect (e.g. a future
# auth gate sending this to a real IdP), cannot run the CI gate this feeds into indefinitely.
# Fix round 2 (SMA-513, Important 1): this call is now GUARDED — without the `|| true`, a page 500
# (an invalid runtime config) or any other non-2xx would abort the script under `set -e` on curl's
# own generic message, after ~20s of retries, with no `::error::` annotation naming the cause. The
# `-w '%{http_code}'` on a status-only probe (`-o /dev/null`, no `-f`) carries the HTTP status into
# the failure message without entangling status-line parsing with the page BODY the second call
# grabs — a combined single-curl capture (body + trailing status code) was considered and rejected
# as the more fragile of the two shapes.
status="$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -L --max-redirs 3 \
  --retry 20 --retry-delay 1 --retry-all-errors "${origin}${base_path}")" || true
if [ "$status" != "200" ]; then
  echo "::error::${image}: ${base_path} did not render (HTTP ${status:-no response}) —" \
    "check the container's runtime configuration (the runtime_env array above) or its logs" >&2
  exit 1
fi
# Second call: the status probe already confirmed 200, but the container can still regress
# between the two calls — a connection drop, a timeout the probe's own retries happened to dodge,
# or `-L` exceeding --max-redirs on an off-origin redirect — so this call is GUARDED too (Fix
# round 3: it previously had none, moving Important 1's defect into a narrower window rather than
# closing it) and carries its OWN, smaller retry budget rather than relying on the probe's.
html="$(curl -fsSL --max-redirs 3 --max-time 30 --retry 5 --retry-delay 1 --retry-all-errors \
  "${origin}${base_path}")" || true
if [ -z "$html" ]; then
  echo "::error::${image}: ${base_path} answered HTTP 200 to the status probe, but the body fetch" \
    "itself failed or returned nothing (connection drop, timeout, or too many redirects) — this is" \
    "a transport failure between the two calls, not a staging or runtime-configuration failure" >&2
  exit 1
fi

# Step 2: extract one chunk URL. No pipe into an early-exit reader: capture, then read.
# `grep -oE` exits 1 on no match even after reading its whole input; under set -euo pipefail an
# unguarded assignment from that pipeline would exit the script here instead of reaching the
# named failure branch below, so the trailing `|| true` hands the empty-string case to the `-z`
# check instead.
chunk="$(printf '%s' "$html" | grep -oE "${base_path}/_next/static/[^\"']+\.js" | sort -u | sed -n 1p)" || true
if [ -z "$chunk" ]; then
  # Fix round 2: Step 1 already confirmed HTTP 200 and followed any redirect, so an invalid
  # runtime configuration and an unfollowed redirect are no longer possible causes HERE — naming
  # them would send a future reader to the wrong place. What is left, given a confirmed 200 page,
  # is a markup or basePath-wiring problem, not a staging or configuration failure.
  echo "::error::${image}: ${base_path} rendered (HTTP 200) but no ${base_path}/_next/static/*.js" \
    "URL appears in the page — check the page markup or the basePath wiring, not staging or" \
    "runtime configuration" >&2
  exit 1
fi

# Step 3: the chunk is served, with a body. When .next/static was never staged, the file is
# simply ABSENT from the served tree, so this is a straight 404, not a 200 with an empty body —
# `curl -fsS` fails outright. Guarded the same way as Step 2's `grep -oE`: without the trailing
# `|| true`, `set -e` would abort the script right here on that 404, before the informative
# message below ever runs.
bytes="$(curl -fsS --max-time 30 "${origin}${chunk}" | wc -c | tr -d ' ')" || true
if [ "${bytes:-0}" -lt 1 ]; then
  # Fix round 2 (Minor): the measured failure here is a 404, not a 200 with an empty body — the
  # wording covers both so the message does not teach the wrong mechanism.
  echo "::error::${image}: ${chunk} is not served (404 or empty body) — .next/static was not staged into the image" >&2
  exit 1
fi

# Step 4: the other zone's prefix does not serve it.
code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 "${origin}${other_prefix}${chunk#"$base_path"}")"
if [ "$code" != "404" ]; then
  echo "::error::${image}: ${other_prefix} also served the chunk (HTTP ${code}); zone asset prefixes collide" >&2
  exit 1
fi

echo "  ${image}: serves ${chunk} (${bytes} bytes), 404 under ${other_prefix}"
