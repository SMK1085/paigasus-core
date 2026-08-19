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
# The trailing `;` is required but need not end the line: a field option (`[deprecated = true]`)
# or a trailing `// comment` is legal proto and must not drop the value from the derived set. It
# would not go unnoticed — the mirror still carries it, so the cross-check reds with "the proto
# registry and its rust mirror disagree" when in fact both are right and only this regex is wrong.
_DECL = re.compile(r"^\s*ERROR_REASON_([A-Z0-9_]+)\s*=\s*\d+\s*(?:\[[^\]]*\])?\s*;", re.MULTILINE)

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
    re-implementation of the mapping rule safe — without it, a parser returning 3 of the registry's
    codes would silently scan for almost nothing and the gate would pass while guarding nothing.
    """
    block = _MIRROR_BLOCK.search(text)
    if block is None:
        raise InfraError(f"EXPECTED_REASONS not found in {REGISTRY_MIRROR} — moved or renamed?")
    return set(_MIRROR_ITEM.findall(block.group(1)))


# Every `.rs` under any `src/` tree below `rs/crates`, at ANY crate depth. Deliberately NOT
# `*/*/src/**/*.rs`: that pins crates to exactly `{libs,bindings,services}/<crate>/`, so a crate
# added one level up would be scheduled by the Moon task (whose `inputs` are the broader
# `rs/crates/**/src/**/*.rs`) yet never scanned — its emission sites invisible and the gate green.
# A gate that fails OPEN is worse than no gate. `self_test` asserts the two scopes still agree.
SCAN_GLOB = "**/src/**/*.rs"
SCAN_ROOT = REPO / "rs/crates"

# Files permitted to spell a registry code.
#   emits    — puts a code on the wire. `guard` names the membership test proving its codes are
#              all declared; check.py asserts that test still exists.
#   asserts  — spells codes without ever putting a literal on the wire: test assertions, or a
#              conversion path that routes every code through an ErrorReason registry static.
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
     "NOT test-only — a production conversion path. Safe because every code leaves it through an "
     "ErrorReason LazyLock static, never through a literal, so a code absent from the registry is "
     "unrepresentable here. It also hosts three membership tests."),
    ("rs/crates/libs/paigasus-proto/src/error.rs", "asserts", None,
     "EXPECTED_REASONS — the registry's own mirror, which this gate cross-checks against"),
    ("rs/crates/services/paigasus-iam/src/adapters/http/error.rs", "asserts", None, "test assertions only"),
    ("rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs", "asserts", None, "test assertion only"),
    ("rs/crates/services/paigasus-iam/src/application/create_user.rs", "asserts", None, "test assertion only"),
    ("rs/crates/libs/paigasus-observability/src/grpc.rs", "excluded", None,
     "grpc_code_name maps tonic::Code to a METRIC LABEL; its \"internal\" collides with the "
     "registry's reason by spelling only. Single-word codes are ordinary English."),
    # No `/*.rs` tail: under fnmatch `**/` still needs a literal `/`, so a file emitted DIRECTLY
    # into `src/generated/` would miss this row and be reported as an offender needing a manifest
    # entry. `…/generated/**` covers both shapes and still contains `_GENERATED_SEGMENT`.
    ("rs/crates/libs/paigasus-proto/src/generated/**", "excluded", None,
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

    The search is tree-wide ON PURPOSE — a row's guard need not live in that row's file. The
    `application/error.rs` row names `every_tenancy_code_is_declared_in_the_canonical_registry`,
    which is defined over in `adapters/grpc/convert.rs`; scoping this lookup to the row's own path
    would red the gate on correct code. Do not "fix" it into a per-row search.

    The match must be a real TEST function, not merely the text `fn <guard>(`. A bare substring
    search also matched a comment mentioning the name, and — worse — still passed after someone
    deleted the `#[test]` attribute, leaving a function nothing runs while this gate reported the
    guard alive (CodeRabbit, owner/repo PR 142).
    """
    decl = _fn_decl_pattern(guard)
    for path in SCAN_ROOT.glob(SCAN_GLOB):
        lines = path.read_text(encoding="utf-8").splitlines()
        for idx, line in enumerate(lines):
            if decl.match(line.strip()) and _is_test_fn(lines, idx):
                return True
    return False


def _fn_decl_pattern(guard):
    """A regex matching a real Rust declaration of `fn <guard>`, anchored at the line start.

    Anchored rather than a `fn <guard>(` substring search, because a substring also matches inside
    a BLOCK comment — and a block comment is invisible to rustc, so

        #[test]
        /* fn every_tenancy_code_is_declared_in_the_canonical_registry( */
        fn unrelated() {}

    applies the attribute to `unrelated`, deletes the real guard, and still reported it alive
    (CodeRabbit, owner/repo PR 142). A line-comment filter did not cover this; anchoring does,
    since `/*` cannot start a declaration.

    Known, accepted false NEGATIVE: a signature preceded on the same line by a closing `*/`, or
    split across lines, is not matched. Both fail SAFE — the gate reds rather than hiding a
    deletion — and `cargo fmt --check` is a mandatory gate on the same PR, so neither survives
    review anyway.
    """
    return re.compile(
        r"^(?:pub(?:\([^)]*\))?\s+)?"
        r"(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+\"[^\"]*\"\s+)?"
        r"fn\s+" + re.escape(guard) + r"\s*[(<]"
    )


# The attribute forms that make a function something the test runner executes. Matched as
# PREFIXES, not exact strings: this repo already writes `#[tokio::test(start_paused = true)]`
# (`adapters/authz/{policy_snapshot,denial_audit}.rs`), and a trailing `// why` comment is
# ordinary. Exact matching would red the gate on a guard that exists and passes — a false alarm on
# correct code, which erodes trust in the gate faster than a missed defect.
_TEST_ATTR_PREFIXES = ("#[test]", "#[test(", "#[tokio::test]", "#[tokio::test(")

# An `#[ignore]`d test never runs, so a guard carrying it is as absent as a deleted one — the exact
# failure `_is_test_fn` exists to close. Prefix, to cover `#[ignore = "reason"]`.
_IGNORE_ATTR_PREFIX = "#[ignore"


def _is_test_fn(lines, idx):
    """Is the `fn` on `lines[idx]` attributed `#[test]` / `#[tokio::test]`?

    Walks backwards over the attribute block directly above the signature. Only attribute lines
    and blank lines may intervene — a doc comment or any code ends the walk, which is what makes
    this a statement about THIS function rather than about a stray attribute higher up the file.

    The WHOLE block is read rather than stopping at the first `#[test]`, because `#[ignore]` may
    sit on either side of it and must veto regardless of order.
    """
    saw_test = False
    for prev in range(idx - 1, -1, -1):
        line = lines[prev].strip()
        if not line:
            continue
        if not line.startswith("#["):
            break
        if line.startswith(_IGNORE_ATTR_PREFIX):
            return False
        if line.startswith(_TEST_ATTR_PREFIXES):
            saw_test = True
    return saw_test


_GLOB_CHARS = "*?["

# The only tree a MANIFEST glob may cover: prost's committed output, which nobody here authors
# and whose filenames follow the proto package rather than a reviewer's choice.
_GENERATED_SEGMENT = "/src/generated/"


def _is_glob(entry):
    """Does this MANIFEST path use fnmatch metacharacters, rather than being a literal path?"""
    return any(ch in entry for ch in _GLOB_CHARS)


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
    # A glob row is the one structural way a later edit could defeat this whole gate. fnmatch's
    # `*` crosses `/`, so a row as innocuous-looking as ("rs/crates/**", "excluded", None, "…")
    # would match EVERY offender — and, because it also matches a hit, would keep every stale-row
    # check green too. Both of check_single_site()'s controls fail open together and it reports
    # nothing, so nothing downstream would ever notice. Bound globs here instead, twice over.
    # The sentinel is a real in-scope emission site AND a literal MANIFEST row, so it cannot drift
    # into an unrepresentative path shape without check_single_site()'s stale-row check saying so.
    sentinel = "rs/crates/services/paigasus-iam/src/adapters/http/authn.rs"
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
        if _is_glob(path):
            # An authored file is named literally; only machine-generated output earns a glob.
            if role != "excluded":
                print(f"  FAIL [manifest] {path}: only an `excluded` row may use a glob", file=sys.stderr)
                rc = 1
            # ...and a glob may cover ONLY machine-generated output. Anything authored is named
            # literally. Without this rule an `excluded` row such as
            # `rs/crates/services/paigasus-gateway/**` silences every unreviewed gateway emission
            # site AND still matches a hit — so the offender check and the stale-row check fail
            # open together, and nothing is left to notice (CodeRabbit, owner/repo PR 142). The
            # sentinel check below cannot catch that one: it is scoped to a single IAM path.
            if _GENERATED_SEGMENT not in path:
                print(f"  FAIL [manifest] glob {path}: a glob may cover only generated output, so its "
                      f"path must contain {_GENERATED_SEGMENT!r} — name authored files literally", file=sys.stderr)
                rc = 1
            # ...and no glob may reach far enough to swallow a real emission site.
            if _matches(sentinel, path):
                print(f"  FAIL [manifest] glob {path} also matches {sentinel} — it is broad enough "
                      "to hide every unlisted emission site", file=sys.stderr)
                rc = 1

    # `_is_test_fn` is what stops `guard_exists` reporting a guard alive after someone deleted its
    # `#[test]`. Exercised against fixtures rather than the tree, so the check keeps biting even
    # once every real guard is correct.
    attributed = ["#[tokio::test]", "async fn every_x() {"]
    if not _is_test_fn(attributed, 1):
        print("  FAIL [_is_test_fn] an attributed async test was not recognised", file=sys.stderr)
        rc = 1
    documented = ["/// doc", "#[tokio::test]", "async fn every_x() {"]
    if not _is_test_fn(documented, 2):
        print("  FAIL [_is_test_fn] a doc comment above the attribute broke recognition", file=sys.stderr)
        rc = 1
    stripped_attr = ["/// doc", "async fn every_x() {"]
    if _is_test_fn(stripped_attr, 1):
        print("  FAIL [_is_test_fn] a fn with its #[test] deleted was still called a test", file=sys.stderr)
        rc = 1
    # This repo really does write the parameterised form; rejecting it would red the gate on a
    # guard that exists and passes.
    for form in ("#[tokio::test(start_paused = true)]", "#[test] // why", '#[test(flavor = "multi_thread")]'):
        if not _is_test_fn([form, "async fn every_x() {"], 1):
            print(f"  FAIL [_is_test_fn] a real test attribute was rejected: {form!r}", file=sys.stderr)
            rc = 1
    # An ignored test never runs, so it must not count — on either side of the attribute.
    for ignored in (["#[test]", "#[ignore]", "fn every_x() {"],
                    ["#[ignore = \"flaky\"]", "#[test]", "fn every_x() {"]):
        if _is_test_fn(ignored, 2):
            print(f"  FAIL [_is_test_fn] an #[ignore]d guard was counted as alive: {ignored[:2]}", file=sys.stderr)
            rc = 1
    distant = ["#[test]", "fn unrelated() {}", "", "fn every_x() {"]
    if _is_test_fn(distant, 3):
        print("  FAIL [_is_test_fn] an attribute belonging to another fn was credited", file=sys.stderr)
        rc = 1

    # `_fn_decl_pattern` is what stops a BLOCK comment counting as a declaration. rustc ignores
    # `/* … */`, so the commented line below is not a guard at all and the `#[test]` above it lands
    # on `unrelated` — the real guard is gone while the gate would call it alive.
    decl = _fn_decl_pattern("every_x")
    for good in ("fn every_x() {", "async fn every_x() {", "pub fn every_x() {", "pub(crate) async fn every_x() {"):
        if not decl.match(good):
            print(f"  FAIL [_fn_decl_pattern] a real declaration was not matched: {good!r}", file=sys.stderr)
            rc = 1
    for bad in ("/* fn every_x( */", "// fn every_x() {", "let _ = fn_every_x();", "*/ fn every_x() {"):
        if decl.match(bad):
            print(f"  FAIL [_fn_decl_pattern] a non-declaration was accepted: {bad!r}", file=sys.stderr)
            rc = 1

    # The scan must reach EVERY `.rs` under a `src/` tree below rs/crates. A narrower glob leaves
    # a crate scheduled-but-unscanned, which is the one failure mode worse than a red: silent.
    reachable = set(SCAN_ROOT.glob(SCAN_GLOB))
    every_src = {p for p in SCAN_ROOT.rglob("*.rs") if "/src/" in p.as_posix()}
    unreachable = sorted(p.relative_to(REPO).as_posix() for p in every_src - reachable)
    if unreachable:
        print(f"  FAIL [scan scope] {len(unreachable)} .rs file(s) under a src/ tree are unreachable "
              f"by SCAN_GLOB, so their emission sites are invisible: {unreachable[:3]}", file=sys.stderr)
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
