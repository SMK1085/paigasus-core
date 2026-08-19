# SPDX-License-Identifier: Apache-2.0
# SMA-553 — repo:* task input-liveness gate.
#
# SMA-541 proves a repo:* gate is WIRED into CI. This is the layer below: a gate that is wired,
# resolvable, and stops firing on the changes it exists to catch, because its `inputs` no longer
# match anything. Moon cannot tell you this itself — `moon query` reports declared inputs VERBATIM,
# unresolved (spec E1), so a glob pointing at a deleted directory reads back exactly as it was
# written. This gate matches every declared pattern against git's tracked set instead.
#
# usage: task_inputs.py [--self-test]
import json
import re
import subprocess
import sys
from pathlib import Path


class GateAssertionError(RuntimeError):
    """An AUTHORIAL mistake -> rc 1, with a message naming what to edit.

    Kept distinct from MoonOutputError so a dead glob (someone moved a directory) can never be
    reported as "re-run the job", which is how a reader triages rc 2 (SMA-541 D2).
    """


class MoonOutputError(RuntimeError):
    """Moon's or git's output did not have the shape this gate requires -> rc 2.

    Raised, never returned as a violation row. A moon upgrade that reshapes the task object must
    fail LOUDLY rather than quietly stop asserting — the drift class this gate exists to close.
    """


INFRA_ERRORS = (
    subprocess.CalledProcessError,
    json.JSONDecodeError,
    OSError,
    MoonOutputError,
)

# Moon injects this onto EVERY task in the graph — all 119 across all 28 projects, including a task
# declaring literally `inputs: []` (spec E2). So a "resolved input set" is never empty, and the
# empty-inputs check (I3) asserts nothing until this is subtracted. D4.
INJECTED_GLOB = ".moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}"

# The conservative charset a pattern must fall inside before it is handed to git. Doubles as the
# pathspec-injection guard: a pattern starting with ':' would be read by git as pathspec magic, and
# the `--` separator plus quoting is necessary but NOT sufficient.
SAFE_CHARS_RE = re.compile(r"[A-Za-z0-9._/*-]+")


def classify(pattern):
    """One pattern's syntactic verdict. PURE — no filesystem, no subprocess.

    Deliberately separate from liveness: this answers "will this gate evaluate the pattern at all",
    check() answers "does it match anything". Splitting them is what lets --self-test drive the
    whole vocabulary without a tree, and it is why a `rejected-*` verdict is a FAILURE rather than
    a skip — a skip is the silent hole this whole gate exists to close.

    The vocabulary is pattern_verdict's (ci/actionlint/run.sh:919-973), deliberately. This is a
    REIMPLEMENTATION, not a reuse: that one is bash, and it answers a different question — whether
    a pattern is legal under GITHUB ACTIONS filter semantics. Keeping the token names identical
    means a reader moving between the two files is not learning a second vocabulary.
    """
    # An exclusion must not be required to match anything; requiring it would be simply wrong.
    # pattern_verdict:928 has this verdict for the same reason. Zero negated globs exist in the
    # graph today, which is exactly why omitting this would have been invisible.
    if pattern.startswith("!"):
        return "negated"
    # git pathspec has NO brace expansion — measured: `:(glob).moon/*.{yml,...}` matches 0 files
    # (spec E4). Expanding braces here would be hand-rolled parsing, "exactly the kind of thing
    # that silently does the wrong thing" (ci/actionlint/run.sh:263-266, about hand-rolled YAML).
    if "{" in pattern or "}" in pattern:
        return "rejected-braces"
    # git and wax equivalence for these is UNMEASURED (spec E4 covers none of them). Unlike
    # pattern_verdict, which rejects them because GitHub's semantics differ, this gate rejects them
    # because nobody has checked. Measuring them is the sanctioned way to lift the restriction.
    if any(ch in pattern for ch in "?+[]"):
        return "rejected-charclass"
    if not SAFE_CHARS_RE.fullmatch(pattern):
        return "rejected-charset"
    segments = pattern.split("/")
    # git normalises './x', 'a/../b' and 'a//b' away when resolving a pathspec. Whether Moon does
    # is unmeasured, so the gate refuses to guess rather than risk disagreeing with it.
    if any(seg in ("", ".", "..") for seg in segments):
        return "rejected-dotty"
    # '**' must be a whole path component; git only honours it as one, and 'a**b' would otherwise
    # be silently downgraded to a single '*'.
    if any("**" in seg and seg != "**" for seg in segments):
        return "rejected-globstar"
    return "ok"


def self_test():
    """Negative control: every assertion must FIRE on a synthetic violation.

    Drives the PURE functions, so no verdict depends on the tree happening to be aligned.
    """
    failures = []

    # --- classify (D6) ------------------------------------------------------------------------
    # Each row is (pattern, expected verdict). Ordering inside classify is load-bearing and these
    # rows pin it: a brace pattern is ALSO outside SAFE_CHARS_RE, and '?' is too, so a
    # reordered implementation would report the generic charset message for both and tell the
    # author nothing actionable.
    for pattern, want in (
        ("ops/observability/prometheus/**/*", "ok"),
        ("rs/**/Cargo.toml", "ok"),
        ("**/*", "ok"),
        ("moon.yml", "ok"),
        ("!ops/scratch/**", "negated"),
        (INJECTED_GLOB, "rejected-braces"),
        ("ts/**/*.{ts,tsx}", "rejected-braces"),
        ("rs/**/*.rs?", "rejected-charclass"),
        ("rs/[abc]/**", "rejected-charclass"),
        ("rs/**/*.jsx+", "rejected-charclass"),
        ("ops/$HOME/**", "rejected-charset"),
        (":(glob)ops/**", "rejected-charset"),
        ("", "rejected-charset"),
        ("./ops/**/*", "rejected-dotty"),
        ("ops/../ops/**/*", "rejected-dotty"),
        ("ops//nats/**", "rejected-dotty"),
        ("rs/a**b/*", "rejected-globstar"),
        ("rs/**x/*", "rejected-globstar"),
    ):
        got = classify(pattern)
        if got != want:
            failures.append(f"classify({pattern!r}) -> {got!r}, expected {want!r}")

    if failures:
        print("task-inputs self-test FAILED:", file=sys.stderr)
        for f in failures:
            print(f"  {f}", file=sys.stderr)
        return 1
    print("task-inputs self-test OK")
    return 0


def main():
    raise NotImplementedError


if __name__ == "__main__":
    sys.exit(self_test() if "--self-test" in sys.argv[1:] else main())
