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
# `grep -oE` exits 1 on no match even after reading its whole input; under set -euo pipefail an
# unguarded assignment from that pipeline would exit the script here instead of reaching the
# named failure branch below, so the trailing `|| true` hands the empty-string case to the `-z`
# check instead.
chunk="$(printf '%s' "$html" | grep -oE "${base_path}/_next/static/[^\"']+\.js" | sort -u | sed -n 1p)" || true
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
