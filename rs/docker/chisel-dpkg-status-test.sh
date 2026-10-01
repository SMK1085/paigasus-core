#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
#
# SMA-665 T2: the self-test of chisel-dpkg-status.sh. It runs in the chisel-dpkg-status-test
# stage of rs/Dockerfile (spec D10), which has the same jq, zstd and dash as the rootfs stage.
#
# Usage: chisel-dpkg-status-test.sh GENERATOR FIXTURE_DIR
#
# One pass row compares the whole status.d tree with FIXTURE_DIR/expected. Each fail row
# changes one input and must see a non-zero exit, its own stderr message (so that a syntax
# error cannot pass as a failure) and no status.d directory left behind.
set -eu

[ "$#" -eq 2 ] || { echo "usage: chisel-dpkg-status-test.sh GENERATOR FIXTURE_DIR" >&2; exit 2; }
gen=$1
fix=$2
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
rows=0
failed=0

ok() {
  rows=$((rows + 1))
  echo "ok   $1"
}

bad() {
  rows=$((rows + 1))
  failed=$((failed + 1))
  echo "FAIL $1: $2" >&2
}

# setup ROW MANIFEST_JSONL CUT_LOG: a fresh root under $work/ROW from the fixture files.
setup() {
  mkdir "$work/$1"
  cp -R "$fix/root" "$work/$1/root"
  mkdir -p "$work/$1/root/var/lib/chisel"
  zstd -q -o "$work/$1/root/var/lib/chisel/manifest.wall" "$2"
  cp "$3" "$work/$1/cut.log"
}

# expect_fail ROW STDERR_FRAGMENT: run the generator on the ROW root, which must fail.
expect_fail() {
  d="$work/$1/root/var/lib/dpkg/status.d"
  before=absent
  if [ -e "$d" ]; then before=present; fi
  if sh "$gen" "$work/$1/root" "$work/$1/cut.log" > "$work/$1/out" 2> "$work/$1/err"; then
    bad "$1" "the generator exited 0"
    return 0
  fi
  if ! grep -F -q -e "$2" "$work/$1/err"; then
    bad "$1" "stderr does not name '$2'; it is: $(cat "$work/$1/err")"
    return 0
  fi
  if [ "$before" = absent ] && [ -e "$d" ]; then
    bad "$1" "the failed run left $d behind"
    return 0
  fi
  ok "$1"
}

m="$fix/manifest.jsonl"
l="$fix/cut.log"
v="$work/inputs"
mkdir "$v"

# Pass: the exact status.d tree, and manifest.wall stays in the image (spec D1, Q4).
setup pass "$m" "$l"
if sh "$gen" "$work/pass/root" "$work/pass/cut.log" 2> "$work/pass/err"; then
  if ! diff -r "$fix/expected" "$work/pass/root/var/lib/dpkg/status.d" > "$work/pass/diff"; then
    bad pass "status.d differs from $fix/expected: $(cat "$work/pass/diff")"
  elif [ ! -f "$work/pass/root/var/lib/chisel/manifest.wall" ]; then
    bad pass "the generator removed manifest.wall"
  else
    ok pass
  fi
else
  bad pass "the generator failed: $(cat "$work/pass/err")"
fi

# Usage and missing inputs.
mkdir "$work/usage"
if sh "$gen" "$work/usage" > /dev/null 2> "$work/usage/err"; then
  bad usage "the generator exited 0 with one argument"
elif ! grep -F -q -e "usage:" "$work/usage/err"; then
  bad usage "stderr does not name 'usage:'"
else
  ok usage
fi

setup no-manifest "$m" "$l"
rm "$work/no-manifest/root/var/lib/chisel/manifest.wall"
expect_fail no-manifest "no chisel manifest"

setup no-cut-log "$m" "$l"
rm "$work/no-cut-log/cut.log"
expect_fail no-cut-log "no chisel cut log"

setup not-zstd "$m" "$l"
cp "$m" "$work/not-zstd/root/var/lib/chisel/manifest.wall"
expect_fail not-zstd "zstd cannot decompress"

# Existing dpkg data (a chisel that writes it itself).
setup status-exists "$m" "$l"
mkdir -p "$work/status-exists/root/var/lib/dpkg"
: > "$work/status-exists/root/var/lib/dpkg/status"
expect_fail status-exists "status already exists"

setup status-d-exists "$m" "$l"
mkdir -p "$work/status-d-exists/root/var/lib/dpkg/status.d"
expect_fail status-d-exists "status.d already exists"

# The jsonwall header.
sed '1s/"count":20/"count":21/' "$m" > "$v/count.jsonl"
setup header-count "$v/count.jsonl" "$l"
expect_fail header-count "its header is not jsonwall 1.0"

sed '1s/"schema":"1.0"/"schema":"2.0"/' "$m" > "$v/schema.jsonl"
setup header-schema "$v/schema.jsonl" "$l"
expect_fail header-schema "its header is not jsonwall 1.0"

# No package object. The count is corrected, so the header check passes and this row reaches
# the package check.
grep -v '"kind":"package"' "$m" > "$v/nopkg.tmp"
sed '1s/"count":20/"count":16/' "$v/nopkg.tmp" > "$v/nopkg.jsonl"
setup no-package "$v/nopkg.jsonl" "$l"
expect_fail no-package "holds no package object"

# The fetch lines (spec D3).
grep -v 'libc6_' "$l" > "$v/nofetch.log"
setup no-fetch-line "$m" "$v/nofetch.log"
expect_fail no-fetch-line "libc6 2.39-0ubuntu8.9 amd64 has 0 matching fetch line(s)"

cp "$l" "$v/twofetch.log"
grep 'libc6_' "$l" >> "$v/twofetch.log"
setup two-fetch-lines "$m" "$v/twofetch.log"
expect_fail two-fetch-lines "libc6 2.39-0ubuntu8.9 amd64 has 2 matching fetch line(s)"

sed 's/libc6_2.39-0ubuntu8.9_amd64/libc6_2.39-0ubuntu8.9_arm64/' "$l" > "$v/arch.log"
setup arch-mismatch "$m" "$v/arch.log"
expect_fail arch-mismatch "libc6 2.39-0ubuntu8.9 amd64 has 0 matching fetch line(s)"

sed 's/libc6_2.39-0ubuntu8.9_amd64/libc6_2.39-0ubuntu8.8_amd64/' "$l" > "$v/version.log"
setup version-mismatch "$m" "$v/version.log"
expect_fail version-mismatch "libc6 2.39-0ubuntu8.9 amd64 has 0 matching fetch line(s)"

cp "$l" "$v/unparsable.log"
echo '2026/10/01 20:09:23 Fetching pool/main/glibc/libc6_2.39-0ubuntu8.9_amd64.deb...' >> "$v/unparsable.log"
setup unparsable-fetch-line "$m" "$v/unparsable.log"
expect_fail unparsable-fetch-line "1 fetch line(s)"

if [ "$failed" -ne 0 ]; then
  echo "chisel-dpkg-status self-test: $failed of $rows rows failed" >&2
  exit 1
fi
echo "chisel-dpkg-status self-test: $rows rows OK"
