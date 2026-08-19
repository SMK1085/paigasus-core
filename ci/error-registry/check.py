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
from fnmatch import fnmatch
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


SCAN_GLOB = "*/*/src/**/*.rs"
SCAN_ROOT = REPO / "rs/crates"

# Files permitted to spell a registry code.
#   emits    — puts a code on the wire. `guard` names the membership test proving its codes are
#              all declared; check.py asserts that test still exists.
#   asserts  — test code only; it checks codes rather than emitting them.
#   excluded — the string is not a registry code here at all. `why` must say why.
# A path may be a literal repo-relative path or a glob.
MANIFEST = (
    ("rs/crates/services/paigasus-iam/src/application/error.rs", "emits",
     "every_tenancy_code_is_declared_in_the_canonical_registry", "TenancyError::code()"),
    ("rs/crates/services/paigasus-gateway/src/adapters/http/error.rs", "emits",
     "every_gateway_code_is_declared_in_the_canonical_registry", "GatewayError::parts()"),
    ("rs/crates/services/paigasus-iam/src/adapters/http/authn.rs", "emits",
     "every_authn_http_code_is_in_the_registry", "the authn funnel and envelope_rejection"),
    ("rs/crates/services/paigasus-iam/src/adapters/http/system_retirement.rs", "emits",
     "every_system_retirement_code_is_declared_in_the_canonical_registry", "the two 409 refusals"),
    ("rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs", "emits",
     "the_terminal_sse_frame_carries_a_registered_code", "the terminal SSE error frame"),
    ("rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs", "asserts", None,
     "hosts three membership tests; emits its codes via ErrorReason LazyLock statics, not literals"),
    ("rs/crates/libs/paigasus-proto/src/error.rs", "asserts", None,
     "EXPECTED_REASONS — the registry's own mirror, which this gate cross-checks against"),
    ("rs/crates/services/paigasus-iam/src/adapters/http/error.rs", "asserts", None, "test assertions only"),
    ("rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs", "asserts", None, "test assertion only"),
    ("rs/crates/services/paigasus-iam/src/application/create_user.rs", "asserts", None, "test assertion only"),
    ("rs/crates/libs/paigasus-observability/src/grpc.rs", "excluded", None,
     "grpc_code_name maps tonic::Code to a METRIC LABEL; its \"internal\" collides with the "
     "registry's reason by spelling only. Single-word codes are ordinary English."),
    ("rs/crates/libs/paigasus-proto/src/generated/**/*.rs", "excluded", None,
     "prost output, not authored here. Its hits are all /// doc comments carried over from "
     "error.proto. Excluded by PATH rather than by a comment filter so a prost change to "
     "#[doc = \"…\"] cannot turn 47 generated lines into offenders overnight."),
)


def code_pattern(codes):
    """Match a code wrapped in either `"…"` or `\\"…\\"`.

    The escaped form is load-bearing: chat.rs builds its terminal SSE frame as one big string
    literal, so its `upstream-error` is surrounded by `\\"`, and a plain-quote anchor misses it
    entirely (SMA-507 E5).
    """
    return re.compile("|".join(r'\\?"' + re.escape(c) + r'\\?"' for c in sorted(codes)))


def scan(codes):
    """{repo-relative path: hit count} for every scanned .rs file that spells a code.

    The WHOLE file is scanned — there is no production/test split. Cutting each file at its first
    column-0 `#[cfg(test)]` was tried and rejected: seven files in this scope open one that is not
    a test module (paigasus-iam/src/config.rs at line 1316 of 3318), which would have silently
    exempted ~4560 production lines (SMA-507 E6).
    """
    pattern = code_pattern(codes)
    hits = {}
    for path in sorted(SCAN_ROOT.glob(SCAN_GLOB)):
        found = len(pattern.findall(path.read_text(encoding="utf-8")))
        if found:
            hits[path.relative_to(REPO).as_posix()] = found
    return hits


def guard_exists(guard):
    """Is `guard` still a test function somewhere under rs/crates?

    Cheap, and it recovers the only load-bearing part of the scheduling task this design dropped:
    deleting a membership test now reds THIS gate, even though nothing here runs it.
    """
    needle = f"fn {guard}("
    return any(needle in p.read_text(encoding="utf-8") for p in SCAN_ROOT.glob(SCAN_GLOB))


def _matches(path, entry):
    """Does repo-relative `path` match a MANIFEST entry (a literal path or a glob)?

    fnmatch, not PurePosixPath.full_match: the latter is 3.13+, and this runs on the CI runner's
    system python3 (the Moon task is toolchain: 'system'). fnmatch's `*` crosses `/`, which is
    what the generated-output glob wants anyway.
    """
    return path == entry or fnmatch(path, entry)


def check_single_site():
    proto = derive_codes(REGISTRY_PROTO.read_text(encoding="utf-8"))
    mirror = mirror_codes(REGISTRY_MIRROR.read_text(encoding="utf-8"))
    if not proto:
        raise InfraError(f"no codes derived from {REGISTRY_PROTO} — the gate would scan for nothing")
    if proto != mirror:
        print("the proto registry and its rust mirror disagree — one of them is wrong:", file=sys.stderr)
        for c in sorted(proto - mirror):
            print(f"    only in error.proto:      {c}", file=sys.stderr)
        for c in sorted(mirror - proto):
            print(f"    only in EXPECTED_REASONS: {c}", file=sys.stderr)
        return 1

    hits = scan(proto)
    if not hits:
        raise InfraError("no code literal found anywhere under rs/crates — pattern or scan root is wrong")

    listed = {path for path, *_ in MANIFEST}
    rc = 0

    offenders = sorted(p for p in hits if not any(_matches(p, entry) for entry in listed))
    if offenders:
        print("file spells a canonical error code but is not on ci/error-registry/check.py's MANIFEST:", file=sys.stderr)
        for p in offenders:
            print(f"    {p}  ({hits[p]} hit(s))", file=sys.stderr)
        print("  add a row (emits/asserts/excluded). An `emits` row needs a membership test.", file=sys.stderr)
        rc = 1

    for path, role, guard, why in MANIFEST:
        if not any(_matches(hit, path) for hit in hits):
            print(f"stale MANIFEST row: {path} no longer spells any code ({why})", file=sys.stderr)
            rc = 1
        if role == "emits" and not guard_exists(guard):
            print(f"MANIFEST names guard `{guard}` for {path}, but no `fn {guard}(` exists", file=sys.stderr)
            rc = 1
    return rc


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
      // metadata carries `capability` (only on ERROR_REASON_CAPABILITY_DISABLED) — a bare
      // constant mention with no `= N;` tail, same shape as the real error.proto at lines 39/162.
      // A de-anchored ERROR_REASON_([A-Z0-9_]+) scan would pick this up as a phantom 4th code.
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

    seen = [path for path, *_ in MANIFEST]
    if len(seen) != len(set(seen)):
        print("  FAIL [manifest] duplicate path rows", file=sys.stderr)
        rc = 1
    for path, role, guard, why in MANIFEST:
        if role not in ("emits", "asserts", "excluded"):
            print(f"  FAIL [manifest] {path} has unknown role {role!r}", file=sys.stderr)
            rc = 1
        if (role == "emits") != (guard is not None):
            print(f"  FAIL [manifest] {path}: exactly the `emits` rows must name a guard", file=sys.stderr)
            rc = 1
        if not why:
            print(f"  FAIL [manifest] {path} has no stated reason", file=sys.stderr)
            rc = 1

    if not code_pattern({"upstream-error"}).search(r'\"code\":\"upstream-error\"'):
        print("  FAIL [code_pattern] the escaped-quote form is not matched (E5 regression)", file=sys.stderr)
        rc = 1

    print("self-test: OK" if rc == 0 else "self-test: FAILED", file=sys.stderr)
    return rc


def main():
    args = sys.argv[1:]
    try:
        if args == ["--self-test"]:
            return self_test()
        if args == ["--single-site"]:
            return check_single_site()
    except InfraError as exc:
        print(f"INFRASTRUCTURE ERROR: {exc}", file=sys.stderr)
        return 2
    except OSError as exc:
        print(f"INFRASTRUCTURE ERROR: {exc}", file=sys.stderr)
        return 2
    print(f"usage: {Path(__file__).name} [--self-test | --single-site]", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
