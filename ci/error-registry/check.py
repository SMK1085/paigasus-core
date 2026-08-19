# SPDX-License-Identifier: Apache-2.0
# SMA-507 — single-site gate for the canonical error-code vocabulary.
#
# Error codes are strings on the wire (google.rpc.ErrorInfo.reason), deliberately not a proto enum,
# so `buf breaking` cannot see them: it does not read kebab strings and never runs against Rust.
#
# WHAT THIS GATES: every file under rs/crates/*/*/src that spells one of the registry's codes must
# be on MANIFEST below. That forces a NEW emission site to be registered and given a membership
# test, which is the failure this repo already had twice (system_retirement.rs had no assertion at
# all; authn.rs's test hand-restated the codes it was meant to check).
#
# WHAT THIS DOES NOT GATE: an UNDECLARED code. The scan greps for the strings the registry
# declares, so a site inventing "widget-jammed" produces no hit. Catching that would mean flagging
# kebab literals by SHAPE, which collides with "content-type"/"application/json" and would drown
# the gate in false positives. See ci/error-registry/README.md.
#
# It never shells out to cargo (the Moon task is toolchain: 'system') and reads no YAML.
#
# usage: check.py [--self-test | --single-site]
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
REGISTRY_PROTO = REPO / "contracts/proto/paigasus/common/v1/error.proto"
REGISTRY_MIRROR = REPO / "rs/crates/libs/paigasus-proto/src/error.rs"


class InfraError(RuntimeError):
    """The inputs or environment are broken, NOT 'the tree regressed'.

    main() maps this to rc 2 so a broken checker aborts loudly instead of folding into a green.
    """


# Matches a declaration line inside `enum ErrorReason`, e.g. `  ERROR_REASON_SLUG_CONFLICT = 1;`.
# Anchored on the `= N;` tail on purpose: a bare ERROR_REASON_[A-Z0-9_]+ scan also matches the
# prefix and the value names error.proto mentions in its own prose comments.
_DECL = re.compile(r"^\s*ERROR_REASON_([A-Z0-9_]+)\s*=\s*\d+\s*;\s*$", re.MULTILINE)

# The `const EXPECTED_REASONS: &[&str] = &[ … ];` block in paigasus-proto's test module.
_MIRROR_BLOCK = re.compile(r"const EXPECTED_REASONS:\s*&\[&str\]\s*=\s*&\[(.*?)\];", re.DOTALL)
_MIRROR_ITEM = re.compile(r'"([a-z0-9-]+)"')


def derive_codes(text):
    """The wire codes error.proto declares, by the mapping rule that file states normatively:
    strip ERROR_REASON_, lowercase, '_' -> '-'. The UNSPECIFIED sentinel is not a code."""
    names = [m.group(1) for m in _DECL.finditer(text)]
    return {name.lower().replace("_", "-") for name in names if name != "UNSPECIFIED"}


def mirror_codes(text):
    """The same vocabulary as transcribed by hand in paigasus-proto/src/error.rs's test module.

    Parsed rather than trusted: this and derive_codes are INDEPENDENT transcriptions of the same
    proto, so they can only agree if both are right. That mutual check is what makes this file's
    re-implementation of the mapping rule safe — without it, a parser returning 3 of 46 codes would
    silently scan for almost nothing and the gate would pass while guarding nothing.
    """
    block = _MIRROR_BLOCK.search(text)
    if block is None:
        raise InfraError(f"EXPECTED_REASONS not found in {REGISTRY_MIRROR} — moved or renamed?")
    return set(_MIRROR_ITEM.findall(block.group(1)))


def self_test():
    """Exercise both parsers against in-process fixtures, so a rotted parser reds even if the real
    tree happens to be clean. Runs FIRST in the Moon task, per moon.yml's affected-smoke and
    publish-metadata precedent."""
    rc = 0

    proto_fixture = """
    enum ErrorReason {
      ERROR_REASON_UNSPECIFIED = 0;
      // "slug-conflict" — mentioned in prose, must not be double-counted
      ERROR_REASON_SLUG_CONFLICT = 1;
        ERROR_REASON_NOT_FOUND = 12;
      ERROR_REASON_INTERNAL = 900;
    }
    """
    got = derive_codes(proto_fixture)
    want = {"slug-conflict", "not-found", "internal"}
    if got != want:
        print(f"  FAIL [derive_codes] {sorted(got)} != {sorted(want)}", file=sys.stderr)
        rc = 1

    mirror_fixture = 'const EXPECTED_REASONS: &[&str] = &[\n // IAM\n "slug-conflict",\n "not-found",\n "internal",\n];'
    got = mirror_codes(mirror_fixture)
    if got != want:
        print(f"  FAIL [mirror_codes] {sorted(got)} != {sorted(want)}", file=sys.stderr)
        rc = 1

    # A truncated mirror must DIFFER from the proto — this is the control that proves the
    # cross-check in check_single_site() can actually fail.
    truncated = mirror_codes('const EXPECTED_REASONS: &[&str] = &["slug-conflict"];')
    if truncated == want:
        print("  FAIL [cross-check control] a truncated mirror compared equal", file=sys.stderr)
        rc = 1

    try:
        mirror_codes("no such const here")
    except InfraError:
        pass
    else:
        print("  FAIL [mirror_codes] a missing EXPECTED_REASONS block did not raise", file=sys.stderr)
        rc = 1

    print("self-test: OK" if rc == 0 else "self-test: FAILED", file=sys.stderr)
    return rc


def main():
    args = sys.argv[1:]
    if args == ["--self-test"]:
        return self_test()
    print(f"usage: {Path(__file__).name} [--self-test | --single-site]", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
