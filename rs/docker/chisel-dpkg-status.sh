#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
#
# SMA-665: writes dpkg status data for the packages of a chisel cut, so that syft's
# dpkg-db-cataloger lists them in the image SBOM. syft 1.52.0 has no cataloger for chisel's own
# manifest (/var/lib/chisel/manifest.wall).
#
# Usage: chisel-dpkg-status.sh ROOT CUT_LOG
#   ROOT     the chisel cut root; it must hold var/lib/chisel/manifest.wall (the
#            base-files_chisel slice).
#   CUT_LOG  the output of `chisel cut`; its `Fetching pool/...` lines give each package's
#            source name.
#
# Writes ROOT/var/lib/dpkg/status.d/<package> (one stanza) and, when the package owns a
# regular file, ROOT/var/lib/dpkg/status.d/<package>.md5sums. This is the distroless layout.
# syft reads the .md5sums file as file ownership: without it, syft's ELF cataloger also reports
# libgcc_s.so.1 as a second package, gcc-14, with a pkg:deb purl and no arch (spec M2, M3).
#
# It fails closed (exit 1, one line on stderr) on any input it does not understand, and then
# writes nothing. It runs under dash: no pipes (dash has no pipefail), no bash syntax.
set -eu

die() {
  echo "chisel-dpkg-status: $*" >&2
  exit 1
}

[ "$#" -eq 2 ] || die "usage: chisel-dpkg-status.sh ROOT CUT_LOG"
root=$1
log=$2
wall="$root/var/lib/chisel/manifest.wall"
dpkg_dir="$root/var/lib/dpkg"

[ -f "$wall" ] || die "no chisel manifest at $wall; the cut needs the base-files_chisel slice"
[ -f "$log" ] || die "no chisel cut log at $log"
# A chisel that starts to write dpkg data itself must not be overwritten or doubled.
[ ! -e "$dpkg_dir/status" ] || die "$dpkg_dir/status already exists; chisel writes dpkg data itself now"
[ ! -e "$dpkg_dir/status.d" ] || die "$dpkg_dir/status.d already exists; chisel writes dpkg data itself now"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

zstd -q -dc "$wall" > "$tmp/manifest.jsonl" || die "zstd cannot decompress $wall"

# The jsonwall header. MEASURED (spec M4, chisel v1.4.2): {"jsonwall":"1.0","schema":"1.0",
# "count":N}, where N is the number of lines INCLUDING the header line.
jq -e -s 'length >= 1 and .[0].jsonwall == "1.0" and .[0].schema == "1.0" and .[0].count == length' \
  "$tmp/manifest.jsonl" > /dev/null \
  || die "the manifest $wall does not parse, or its header is not jsonwall 1.0, schema 1.0 with a count equal to its line count"

jq -r -s '.[1:][] | select(.kind == "package") | "\(.name) \(.version) \(.arch)"' \
  "$tmp/manifest.jsonl" > "$tmp/packages" || die "jq cannot read the package objects of $wall"
[ -s "$tmp/packages" ] || die "the manifest $wall holds no package object"

# Every fetch line must parse. The source name is the pool directory (spec D3):
#   Fetching pool/<component>/<prefix>/<source>/<name>_<version>_<arch>.deb...
grep -oE 'Fetching pool/[^ ]*' "$log" > "$tmp/fetch.raw" || true
sed -n 's#^Fetching pool/[^/]*/[^/]*/\([^/_]*\)/\([^/_]*\)_\([^/_]*\)_\([^/_.]*\)\.deb\.\.\.$#\2 \3 \4 \1#p' \
  "$tmp/fetch.raw" > "$tmp/fetch"
raw_n=$(wc -l < "$tmp/fetch.raw")
ok_n=$(wc -l < "$tmp/fetch")
[ "$raw_n" -eq "$ok_n" ] \
  || die "$((raw_n - ok_n)) fetch line(s) in $log do not parse as pool/<component>/<prefix>/<source>/<name>_<version>_<arch>.deb"

out="$tmp/status.d"
mkdir "$out"
chmod 0755 "$out"
while read -r name version arch; do
  # A pool file name has no epoch: 1:1.3.dfsg-3 is fetched as <name>_1.3.dfsg-3_<arch>.deb. The
  # stanza keeps the full version, as dpkg does.
  case $version in
    *:*) pool_version=${version#*:} ;;
    *) pool_version=$version ;;
  esac
  match=$(awk -v n="$name" -v v="$pool_version" -v a="$arch" \
    '$1 == n && $2 == v && $3 == a { c++; s = $4 } END { print c + 0, s }' "$tmp/fetch")
  count=${match%% *}
  source=${match#* }
  [ "$count" -eq 1 ] \
    || die "package $name $version $arch has $count matching fetch line(s) in $log; expected exactly 1"
  {
    printf 'Package: %s\n' "$name"
    printf 'Status: install ok installed\n'
    printf 'Architecture: %s\n' "$arch"
    printf 'Version: %s\n' "$version"
    # dpkg omits Source: when the source name equals the package name.
    if [ "$source" != "$name" ]; then
      printf 'Source: %s\n' "$source"
    fi
  } > "$out/$name"
  # The regular files of the package's slices (a slice name is <package>_<slice>). A directory
  # or a symlink has no sha256 in the manifest.
  jq -r --arg p "$name" \
    'select(.kind == "path" and has("sha256") and any(.slices[]; split("_")[0] == $p)) | .path' \
    "$tmp/manifest.jsonl" > "$tmp/paths" || die "jq cannot read the path objects of $wall"
  : > "$tmp/md5"
  while read -r path; do
    sum=$(md5sum "$root$path") || die "md5sum cannot read $root$path"
    printf '%s  %s\n' "${sum%% *}" "${path#/}" >> "$tmp/md5"
  done < "$tmp/paths"
  if [ -s "$tmp/md5" ]; then
    LC_ALL=C sort -k 2 "$tmp/md5" > "$out/$name.md5sums"
  fi
done < "$tmp/packages"

mkdir -p "$dpkg_dir"
mv "$out" "$dpkg_dir/status.d"
