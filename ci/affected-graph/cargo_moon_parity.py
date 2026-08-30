# SPDX-License-Identifier: Apache-2.0
# SMA-524 — Cargo <-> Moon dependency-graph parity gate.
#
# The affected-graph guard (SMA-409/429) asserts only the edges someone remembered to write a CASE
# for. SMA-505 added a crate with no case, so its three missing edges survived a full review cycle.
# This gate is generic: it compares every crate's Cargo dependencies against Moon's OWN RESOLVED
# graph, so a new crate cannot repeat that failure.
#
# It never parses moon.yml (formatting-proof) and never shells out to cargo (repo:affected-smoke is
# toolchain: 'system', so cargo is not a dependency this script may take). Cargo.toml is parsed with
# tomllib, which — unlike the regex this replaced — handles dotted keys
# (`paigasus-kernel.workspace = true`), inline tables, and `package =` renames. That regex reported
# five sound edges as phantom.
#
# It also carries A8 (SMA-601), which is about the FLAGS rather than the graph: every task whose
# resolved invocation reaches cargo must pass --locked, because an unlocked one re-resolves and
# rewrites an inconsistent Cargo.lock in place. That is how five Dependabot PRs merged a truncated
# lock through a green required check.
#
# It also carries A4 (SMA-534), which is about task INPUTS rather than edges: every crate's `lint`
# must key on the workspace-level files (Cargo.lock, Cargo.toml, rust-toolchain.toml and, since
# SMA-594, .cargo/config.toml), since `rs/` has no Moon project for a dependency edge to point at. A4 reads moon's RESOLVED `inputFiles`, so
# it stays inside the "never parse YAML" rule above.
#
# It also carries A9 (SMA-604), the only assertion here about a consumer OUTSIDE this repo:
# Dependabot resolves `rs/Cargo.toml`'s `[workspace] members` with its own expander, which is
# weaker than Cargo's. A glob Cargo reads fine can resolve to ZERO members there, which silently
# shrinks Dependabot's sandbox to whatever `path =` deps it can still reach — and a shrunken
# sandbox both truncates the lock it proposes and reds its own job.
#
# usage: cargo_moon_parity.py [--self-test]
import collections
import fnmatch
import inspect
import json
import os
import re
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path


class MoonOutputError(RuntimeError):
    """Moon's query output did not have the shape this gate requires.

    Raised — never returned as a violation row — when moon reports a task with none of a `command`,
    a `script`, or any `args` (moon_projects() joins all three into the text A5 marker-matches
    against, so only the absence of all three counts). That is "moon told us nothing", which must
    abort as an infrastructure error (rc 2) rather than be folded into an assertion failure, exactly
    as A4 treats an absent `inputFiles` key. A moon upgrade that reshapes the task object must fail
    loudly, not quietly stop asserting.
    """


# Exceptions meaning "the inputs or environment are broken", NOT "the graph regressed". main() maps
# these to rc 2 so run.sh aborts, instead of folding them into SUITE_RC as an assertion failure.
# tomllib.TOMLDecodeError subclasses ValueError, not OSError, so it has to be named explicitly — the
# self-test pins that by asserting cargo_crates' real failure is a member of this tuple.
INFRA_ERRORS = (
    subprocess.CalledProcessError,
    json.JSONDecodeError,
    tomllib.TOMLDecodeError,
    OSError,
    MoonOutputError,
)

# (consumer, upstream) -> why this hand-declared Moon edge has no Cargo backing.
# An allowlisted edge is a RECORDED DECISION, not a silent exemption: the reason string is required.
ALLOW_NO_CARGO_BACKING = {
    ("paigasus-gateway-rs", "paigasus-kernel-rs"): (
        "Over-approximation, not a defect: the gateway has no Cargo dep on the kernel. Removing the "
        "edge would change the kernel->bindings expected set that SMA-409 owns (SMA-524 D4). NOTE "
        "(SMA-528): this is no longer free. The edge now feeds @group(upstreams), so every kernel "
        "edit runs the gateway's full build+test+lint for a dependency that does not exist. Revisit "
        "if kernel PRs approach the CI budget — that is the first thing to drop."
    ),
}

# Build-scope parents injected by a task dep (e.g. `contracts:generate`), never Cargo deps.
# Used twice: A2 must not report them as unbacked Moon edges, and A6's closure walk must not demand
# `src/**/*` + `Cargo.toml` globs for them — they have neither tree (SMA-528).
NON_CARGO_PARENTS = {"contracts"}

# SMA-534 — the workspace-level files `lint` must key on. `rs/` has no Moon project, so without
# these declared on the inherited lint task a Cargo.lock-only change (every Dependabot Cargo PR)
# schedules no crate task at all. Paths are workspace-relative, exactly as Moon RESOLVES them:
# the YAML says `/rs/Cargo.lock`, `moon query projects` reports `rs/Cargo.lock`.
#
# SMA-594 adds the fourth. Cargo finds `.cargo/config.toml` by walking up from the WORKING
# DIRECTORY, and every cargo invocation in this repo runs with cwd inside `rs/`, so the file is
# read by all of them. It sets `rustflags` for the two *-apple-darwin targets. The criterion for
# a cache input is "does this influence the output", not "is it strictly required" — which is why
# it goes on every cargo-from-rs/ task and not only the two that need the flags today. This
# REVERSES SMA-546's deliberate exclusion; see the design doc's D1 and §3.4 for the argument.
WORKSPACE_LINT_INPUTS = (
    "rs/Cargo.lock",
    "rs/Cargo.toml",
    "rs/rust-toolchain.toml",
    "rs/.cargo/config.toml",
)

# SMA-546 — A5. The tasks that COMPILE the FFI cdylibs live in the ts/py stacks, so A4's
# per-crate loop cannot reach them: `moon query projects` lists them under their own project ids,
# not under any Rust crate. They must key on the same workspace files as `lint`, plus `.prototools`
# — which pins `wasm-pack` and is therefore the OTHER half of the rs/Cargo.toml:90-97 invariant
# ("the pinned wasm-pack must support that 0.2.z — bump the two together").
FFI_TASK_INPUTS = (*WORKSPACE_LINT_INPUTS, ".prototools")

# SMA-537 — what every crate's `fmt` must key on. The two globs come from the shared fileGroups
# and land in moon's `inputGlobs`; the two literals land in `inputFiles`. check_task_inputs spans
# both, which is the whole reason it does not read a single bucket the way its lint-only ancestor did.
FMT_TASK_INPUTS = (
    "rs/rustfmt.toml",
    "rs/rust-toolchain.toml",
    "Cargo.toml",
    "src/**/*",
    "tests/**/*",
)

# Substrings that mean "this task shells out to a Rust build". Matched against the task's resolved
# `command` + `args` + `script` joined — NOT `command` alone: measured on moon 2.5.3, a
# command-form task reports command='cargo' with the verb in args (paigasus-kernel-rs:lint ->
# args=['clippy', '--locked', ...], script=None), so a `command: 'napi'` + `args: ['build', ...]`
# task would be invisible to a command-only scan.
#
# `maturin` is FORWARD-LOOKING and matches no RESOLVED moon task invocation today: the string
# shows up elsewhere in the tree (pyproject.toml, moon.yml comments, the python template), but
# paigasus-kernel-py:test's actual resolved script is
# `uv sync --reinstall-package paigasus-py-bindings`. It is kept so a future direct maturin
# invocation is covered on day one. Do not mistake it for measured coverage.
FFI_MARKERS = ("napi build", "wasm-pack", "maturin", "--reinstall-package")

# The floor. A5's derived set is its strength (a fourth FFI task is covered the day it is added —
# SMA-524's "a MISSING case is how the bug survived" lesson) and also its weakness: a derived set
# that shrinks to EMPTY asserts nothing while still printing PASS. Moving an invocation behind a
# package.json script, `--reinstall-package` becoming `--refresh-package`, or a moon upgrade
# renaming the `script` key would each do that silently. A4's "absent inputFiles is a violation"
# rule does NOT protect against it — when nothing matches, inputFiles is never consulted.
# So: every task named here MUST be in the derived set, or A5 fails.
REQUIRED_FFI_TASKS = (
    "paigasus-kernel-py:test",
    "paigasus-kernel-ts:build",
    "paigasus-kernel-ts:test",
)

# SMA-601 — the cargo subcommands that RESOLVE the dependency graph, and therefore rewrite an
# inconsistent Cargo.lock in place unless --locked is passed. `fmt` and `machete` are absent
# deliberately: neither reads the lock.
#
# The list holds two kinds of verb. The first kind resolves as a side effect of doing something
# else (`build`, `test`, `tree`, …). The second kind exists to WRITE the lock: `add`, `remove`,
# `generate-lockfile`, `vendor` and `fix`. None of the second kind is used in this repo today,
# and `--locked` is meaningless or rejected for some of them — which is the point. A8 must report
# such a verb rather than pass it over in silence, so that a future `cargo add` inside a Moon task
# is a reviewed ALLOW_UNLOCKED_CARGO entry and not an unnoticed lock repairer.
#
# Matched against the same resolved `command` + `args` +
# `script` blob A5 uses, NOT against file text — a text scan of moon.yml/.moon/tasks/*.yml/
# rs/Dockerfile/ci/**/*.sh was measured at 45 matches of which ~14 were real invocations, because
# `moon.yml:323` is `echo "cargo tree failed ..."` on an EXECUTING line and
# `ci/publish-metadata/run.sh:179` is a Python f-string inside a heredoc. The resolved blob is not
# prose-free either — `repo:wasm-getrandom-free`'s own script carries that same
# `echo "cargo tree failed ..."` and the regex matches it — but it holds no prose about a task
# OTHER than itself, which is what makes the TASK-level count clean: measured at 60 matched tasks
# (57 literal-cargo, 3 wrapper), 0 false positives.
LOCK_RESOLVING_VERBS = (
    "add", "bench", "build", "check", "clippy", "deny", "doc", "fetch", "fix",
    "generate-lockfile", "metadata", "nextest", "package", "publish", "remove",
    "run", "test", "tree", "update", "vendor",
)

# NOT the whole story since SMA-605: this is the LITERAL arm only. `cargo_matches` merges it with
# two INDIRECT arms — a cargo-named variable in command position (CARGO_VAR_CMD_RE) and the
# `CARGO=` environment prefix (CARGO_ENV_PREFIX_RE) — and every consumer reads that merged list,
# not this regex. A10 has its own sensitive-verb variant, CARGO_VAR_CMD_SENSITIVE_RE.
CARGO_INVOCATION_RE = re.compile(
    r"\bcargo\s+(?:\+\S+\s+)?(?:" + "|".join(LOCK_RESOLVING_VERBS) + r")\b"
)

# `--locked` is accepted; `--frozen` is NOT — it implies `--offline`, which false-reds on a cold
# cargo cache, the same reason this gate refuses `--offline` elsewhere.
LOCKED_FLAG = "--locked"

# The `bash`/`sh` prefix is OPTIONAL. Measured: five of the eight invoked gate scripts are
# called BARE (`ci/release-parity/run.sh --ecosystem semantic-release`), so a prefix-requiring
# extractor sees 3 of 8 scripts — which is exactly the error the first draft of the SMA-599
# spec made, and it invalidated its own "zero false positives" measurement.
SCRIPT_REF_RE = re.compile(r"(?:^|[\s;&|(])(?:bash\s+|sh\s+)?(ci/[\w./-]+\.sh)\b")

# SMA-599 — A10's verb predicate, deliberately NOT LOCK_RESOLVING_VERBS.
#
# The two lists answer different questions. A8 asks "does this resolve the lock"; A10 asks
# "can rs/.cargo/config.toml change this command's OUTPUT". Reusing A8's list made A10 fail to
# implement its own rule: the thirteen `cargo fmt --check` tasks run with cwd inside `rs/` and
# fell out of scope only because `fmt` happens to be absent from a lock-oriented list — an
# accidental coupling, not a stated exclusion.
#
# CORRECTED CLAIM (SMA-599 review): CONFIG_SENSITIVE_VERBS is a STRICT SUBSET of
# LOCK_RESOLVING_VERBS (A8's list), and the split is load-bearing ONLY for verbs
# LOCK_RESOLVING_VERBS already derives — it is NOT a general fix for "a future compiling
# subcommand". Both `derive_cargo_tasks` and `script_cargo_lines` gate on CARGO_INVOCATION_RE
# first, which is built from LOCK_RESOLVING_VERBS, not from this list. So `cargo llvm-cov`,
# `insta`, `udeps`, `bloat` or `tarpaulin` yield an EMPTY derivation today and A10 never examines
# them — no row, and no `FLOOR:` row either, since the floor only sees what's derived (spec
# L11). Widening the derivation to catch those verbs is a separate, larger change; this
# predicate only narrows what's ALREADY derived.
#
# In: subcommands that COMPILE or LINK, so the two *-apple-darwin rustflags reach them.
# Out, each for a stated reason:
#   fmt                 formats; neither compiles nor links (.moon/tasks/rust.yml:125-149)
#   tree, metadata      resolve the graph; never compile (this is AC 4's `cargo tree`
#                       exclusion, encoded in the predicate so a FUTURE cargo-tree gate, if
#                       ever added to LOCK_RESOLVING_VERBS' derivation, is excluded on day one
#                       rather than needing its own waiver)
#   deny, machete       third-party static scans over the manifest and lock
#   add/remove/update/generate-lockfile/vendor/fetch   lock manipulation, no build
CONFIG_SENSITIVE_VERBS = (
    "bench", "build", "check", "clippy", "doc", "fix", "nextest",
    "package", "publish", "run", "test",
)
CONFIG_SENSITIVE_RE = re.compile(
    r"\bcargo\s+(?:\+\S+\s+)?(?:" + "|".join(CONFIG_SENSITIVE_VERBS) + r")\b"
)
# SMA-605 — A10's OWN arm 1. Built from CONFIG_SENSITIVE_VERBS, never from
# LOCK_RESOLVING_VERBS: A10 asks "can rs/.cargo/config.toml change this command's OUTPUT", and
# reusing A8's list would pull `"$CARGO_BIN" tree` / `deny` / `update` into A10's scope with
# nothing to red it — the coupling SMA-599 D9 removed for the literal arm, re-created for the
# indirect one.
CARGO_VAR_CMD_SENSITIVE_RE = re.compile(
    r"""(?:^|[\s;&|(])["']?\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?["']?[^\S\n]+"""
    r"(?:\+\S+[^\S\n]+)?(?:" + "|".join(CONFIG_SENSITIVE_VERBS) + r")\b"
)
CARGO_CONFIG_INPUT = "rs/.cargo/config.toml"

# SMA-605 — the two INDIRECT arms, beside CARGO_INVOCATION_RE's literal one.
#
# FORWARD COVER, NOT MEASURED COVERAGE — the same warning FFI_MARKERS carries for `maturin`.
# Arm 1 reports ZERO rows on the real corpus and always has; arm 2 reports exactly one, at
# ci/release-parity/ecosystems/release-plz.sh:152, and only once script_source_refs makes that
# file reachable. Do not read a green run as proof either arm works — the self-test fixtures are
# the proof.
#
# Arm 1 — a cargo-NAMED variable in command position. The NAME is the whole test (spec R1).
# Value resolution was measured and rejected: VAR_ASSIGN_RE captures `$(` as the value of
# CARGO_BIN="$( command -v cargo … )", so it cannot reach the real shape, and a value predicate
# would fire on the three variables in ci/actionlint/run.sh whose literal values mention cargo —
# the file SMA-599 L4 already names as one edit from a spurious row.
# HORIZONTAL whitespace between the variable and its verb, never `\s` — same reason as arm 2's
# lookahead (M5): `\s` crosses a physical line, COMMAND_SPLIT_RE does not split on newlines, and a
# blob is often a multi-line `script:` block, so `"$CARGO_BIN"` on one line and `build` on the
# next would read as one invocation. The LEADING separator keeps `\s`: a newline really does start
# a command.
CARGO_VAR_CMD_RE = re.compile(
    r"""(?:^|[\s;&|(])["']?\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?["']?[^\S\n]+"""
    r"(?:\+\S+[^\S\n]+)?(" + "|".join(LOCK_RESOLVING_VERBS) + r")\b"
)
CARGO_VAR_NAME = "cargo"

# Arm 2 — the `CARGO=` environment prefix, the shape this repo actually uses
# (ci/release-parity/ecosystems/release-plz.sh:152). The name is EXACTLY `CARGO`: CARGO_HOME,
# CARGO_TERM_COLOR and CARGO_NET_OFFLINE configure cargo without redirecting it, and line 152
# carries both `CARGO=` and `CARGO_NET_OFFLINE=` so a "name mentions cargo" predicate reports the
# wrong one (spec M6).
#
# No verb requirement: the tool's verbs belong to the tool, not to cargo. The trailing word is a
# LOOKAHEAD, never consumed — that is what makes `export CARGO=/p`, an assignment with nothing to
# run, report nothing, and it keeps a second env prefix's leading separator intact.
# The trailing lookahead is HORIZONTAL whitespace only (`[^\S\n]`), never `\s`. MEASURED: with
# `\s` it crosses a newline, so `export CARGO=/p` followed by an unrelated command on the NEXT
# line matches — and fifteen real moon blobs are multi-line `script:` blocks, so that is a live
# false positive on the blob arm, not a hypothetical one. The LEADING separator keeps `\s`
# deliberately: a newline before `CARGO=` really does start a command.
# The lookahead skips any further `NAME=value` assignments and then demands a NON-assignment
# token: `CARGO=/p CARGO_HOME=/x` sets two variables and runs nothing, so it is not a wrapper.
# Skipping rather than simply rejecting an assignment is what keeps `CARGO=/p CARGO_HOME=/x tool
# run` matched — the tool is still reached with cargo redirected.
_ENV_ASSIGN = r"""[A-Za-z_][A-Za-z0-9_]*=(?:"[^"]*"|'[^']*'|[^\s;&|]*)"""
CARGO_ENV_PREFIX_RE = re.compile(
    r"""(?:^|[\s;&|(])CARGO=(?:"[^"]*"|'[^']*'|[^\s;&|]*)"""
    r"(?=(?:[^\S\n]+" + _ENV_ASSIGN + r")*[^\S\n]+(?!" + _ENV_ASSIGN + r")\S)"
)

# A Dockerfile `ENV CARGO=…` is NOT a shell env prefix and deliberately does not go through
# CARGO_ENV_PREFIX_RE: it carries no command on its own line, yet it redirects cargo for every
# later RUN in the image. `\bCARGO=` does not match `CARGO_HOME=`, since the `=` there follows
# `HOME` (CodeRabbit PR review asked for this case to be handled separately, and it is).
DOCKERFILE_ENV_RE = re.compile(r"^\s*ENV\b", re.I)
# `CARGO` must be an assignment KEY. Quoted spans are blanked first, so
# `ENV LABEL="CARGO=/usr/bin/cargo"` — which sets no such variable — does not report. A bare
# `\bCARGO=` over the raw line DID report it, and would have red CI on a benign Dockerfile
# (MEASURED, CodeRabbit PR review).
DOCKERFILE_ENV_CARGO_KEY_RE = re.compile(r"(?:^|\s)CARGO=")


def _dockerfile_env_redirects_cargo(stripped):
    """True when a Dockerfile ENV directive assigns CARGO itself."""
    if not DOCKERFILE_ENV_RE.match(stripped):
        return False
    masked = SHELL_STRING_RE.sub(lambda m: " " * len(m.group(0)), stripped)
    return bool(DOCKERFILE_ENV_CARGO_KEY_RE.search(masked))

# Module level, not rebuilt per call: `cargo_matches` runs ~41k times on the real corpus.
ENV_ONLY_GAP_RE = re.compile(r"^(?:[^\S\n]+" + _ENV_ASSIGN + r")*[^\S\n]*$")

CargoMatch = collections.namedtuple("CargoMatch", "start end verb kind")

# Only these tokens confer a cwd. A bare `rs`-containing ARGUMENT must never do so:
# `cargo deny --manifest-path rs/Cargo.toml` and `cargo machete rs` both mention `rs` and both
# run from the repo ROOT. MEASURED on cargo 1.95.0 (SMA-599 §2.3): with rs/.cargo/config.toml
# made malformed, cwd=rs/ fails at rc 101 while cwd=root with --manifest-path succeeds at rc 0,
# so --manifest-path does NOT move cargo's config walk.
# A leading `--` option terminator (`cd -- rs`) is skipped, not captured as the target —
# SMA-599 review, MEASURED false negative before this fix: without the optional group the
# captured token was the literal string "--", which never matches RS_PATH_RE.
CWD_TOKEN_RE = re.compile(r"(?:\bcd\b|\bpushd\b|--cwd)\s+(?:--\s+)?[\"']?([^\"'\s;&|)]+)")
# Command substitution `$(...)` breaks CWD_TOKEN_RE's character class above (it excludes `)`,
# needed so a bare `rs`-containing ARGUMENT never confers scope — see the comment there).
# `_cwd_inside_rs` deletes non-nested `$(...)` spans before tokenizing, so a target like
# `"$(git rev-parse --show-toplevel)/rs"` still reads as ending in `/rs` once the substitution
# is removed — SMA-599 review, MEASURED false negative before this fix (the un-fixed regex
# truncated the captured token at the FIRST `)`, which lands inside the substitution, losing
# the `/rs` suffix entirely). Nested substitution is not handled.
CMD_SUBST_RE = re.compile(r"\$\([^()]*\)")
# One round of literal substitution, enough for `RS_DIR="$REPO_ROOT/rs"` … `cd "$RS_DIR"`
# (ci/publish-metadata/run.sh:89,1654). Both `$VAR` and `${VAR}` forms. STATED LIMIT (spec L5,
# SMA-599 review): two-level indirection — `A=rs; B="$A"; cd "$B"` — needs a SECOND round and
# stays a false negative. Substituting `B` yields the literal text `$A`, which this function
# never re-scans for a remaining `$A`/`${A}`. No live script does this today; if one starts to,
# this is a silent false negative, not a crash — left unfixed deliberately rather than adding a
# fixed-point loop for a shape that does not exist yet.
VAR_ASSIGN_RE = re.compile(r"""(?m)^\s*([A-Za-z_][A-Za-z0-9_]*)=["']?([^"'\s]+)["']?""")
RS_PATH_RE = re.compile(r"(?:^|/)rs(?:/|$)")

# A10's waivers. EMPTY, like ALLOW_OVER_APPROXIMATION: every exclusion is structural, via the
# verb predicate or the cwd rule. An entry needs a non-empty reason, and an entry naming a task
# outside the examined set is itself a row.
ALLOW_MISSING_CARGO_CONFIG = {}

# A10's floor. Members must be IN SCOPE and NOT allowlisted — a default-deny gate has a second
# vacuity mode the FFI floors do not: an allowlist that grows to swallow the derived set.
REQUIRED_CARGO_CONFIG_TASKS = (
    "paigasus-kernel-rs:build",
    "paigasus-iam-rs:test",
    "paigasus-kernel-ts:build",
    "repo:parity-corpus-drift",
    "repo:publish-metadata",
)

# SMA-599 — the shell-script cargo-line classifier shared by A8's script arm and A10.
#
# THE CONSERVATIVE RULE. Report every cargo invocation that does not carry `--locked` in its
# own tail — the text from its verb to the next invocation in the same command segment, or to
# the end of that segment. Exactly three regions are excluded, because in each the shell
# provably never executes the text as a command: a heredoc BODY, a `#` comment tail, and a
# BRACKETED OPERATOR SPAN. The last one is three shapes, not one — `$(( ... ))`, a bare
# `(( ... ))` arithmetic command, and `[ ... ]` (an array subscript, a `[[ ]]` test, a glob) —
# and it is blanked in the MASK only, so a `<<` there is a shift and a `#` a base marker while
# the code text itself survives verbatim and still classifies (see `_blank_operator_spans`).
# Nothing else is excluded. In particular quoted string literals are NOT stripped, so a cargo
# verb sitting inside a string reports like any other.
#
# WHY, and what this replaced (SMA-599). The first implementation stripped quoted strings and
# then tried to decide, per line, whether a verb inside one still executed: `bash -c "..."`,
# `eval`, a `$( ... )` body, a quote span crossing physical lines. Four layers, 441 lines, of
# which the quote-span tracker alone was 196 with six states feeding three consumers. Three
# review rounds each found a different SILENT FALSE NEGATIVE, and rounds 2-4 were each an
# interaction between one layer and the layer added before it. The last one, measured against
# real bash:
#
#     bash -c \
#       "cargo build"
#
# reported zero rows and no error while real bash runs cargo unlocked, because the exec-vs-
# plain decision read the RAW physical line while continuation joining happened later, on the
# LOGICAL line. A gate whose defects are silent passes cannot converge.
#
# The conservative rule has ONE decision, so a future defect can only be a FALSE POSITIVE: a
# benign string that mentions a cargo verb reports, CI reds loudly, and a reviewer adds a
# waiver. A default-deny gate is built on exactly that asymmetry.
HEREDOC_OPEN_RE = re.compile(r"<<-?\s*(['\"]?)([A-Za-z_][A-Za-z0-9_]*)\1")
# Used ONLY to decide WHICH OFFSETS of a physical line the shell executes, never to remove
# content: matched quote pairs are blanked to equal-length runs of spaces so every surviving
# offset still indexes the original, and `_line_regions` slices the ORIGINAL. Both line-local
# exclusions read that one mask — the `#` comment cut and the heredoc-opener scan — because a
# `<<EOF` inside a string is no more an opener than a `#` inside one is a comment.
SHELL_STRING_RE = re.compile(r"'[^']*'|\"[^\"]*\"")
# Command separators. `--no-deps` is read within the segment holding the cargo verb, and
# `--locked` within that segment AFTER the verb — so `cargo build && cargo metadata --locked`
# does NOT count as locking `cargo build`, and a `--locked` that is string content preceding
# the verb does not either.
COMMAND_SPLIT_RE = re.compile(r"[;&|]+")
CARGO_METADATA_RE = re.compile(r"\bcargo\s+(?:\+\S+\s+)?metadata\b")

ScriptCargoLine = collections.namedtuple(
    "ScriptCargoLine", "lineno raw segment resolves locked kind"
)

# A wrapper reaches cargo without the literal token, so A8 matches FFI_MARKERS too. Without this
# the three wrapper tasks would be silently OUT of scope rather than visibly allowlisted.
#
# The two match kinds are NOT interchangeable, and conflating them is vacuous (SMA-601, measured).
# A literal `cargo <verb>` match is satisfied by `--locked` appearing in the blob, because that
# blob IS the invocation. A WRAPPER match is not: `paigasus-kernel-ts:build` runs `napi build`
# AND `wasm-pack build ... -- --locked` in one script, so a blob-level `--locked` test greens the
# task while `napi build` still re-resolves and repairs the lock. A wrapper-matched task therefore
# ALWAYS needs an ALLOW_UNLOCKED_CARGO entry, whether or not `--locked` appears anywhere in its
# blob; a task matching both kinds is governed by the wrapper rule, which is the stricter one.
ALLOW_UNLOCKED_CARGO = {
    "paigasus-kernel-ts:build": (
        "reaches cargo through TWO wrappers, neither of which can guarantee a locked resolution "
        "(both measured, SMA-601). `napi build` exposes no --locked and no cargo passthrough, and "
        "cargo has no env-var equivalent. `wasm-pack build ... -- --locked` DOES forward the flag "
        "to the cargo build it wraps, but wasm-pack makes its OWN unlocked cargo call BEFORE that "
        "build and repairs the lock there: measured against a truncated 176-package lock it exits "
        "0 and rewrites the lock to 548. The flag is kept anyway — it constrains the forwarded "
        "build — but it does not lock the task."
    ),
    "paigasus-kernel-ts:test": "as paigasus-kernel-ts:build",
    "paigasus-kernel-py:test": (
        "reaches cargo through `uv sync --reinstall-package`, which drives maturin, which drives "
        "cargo — no flag path through either (SMA-601)"
    ),
}

# SMA-599 — waivers for cargo lines inside a gate's own script. Keyed by
# (script path, stripped segment text) and NOT by line number: a line-number key would red
# repo:affected-smoke on any unrelated insertion above the line, in a 620-line file that
# SMA-576 and SMA-579 both edited. The uniqueness assertion is what makes text safe — a text
# occurring twice is ambiguous and is reported rather than silently covering both.
#
# A stale entry (text no longer present) is a row, the stale-skip idiom
# ci/actionlint/run.sh:2376-2383 already uses.
ALLOW_UNLOCKED_CARGO_SCRIPT = {
    ("ci/version-lockstep/run.sh", "cargo update -w --offline >/dev/null 2>"): (
        "MEASURED unreachable from the Moon task (SMA-599 §2.4): repo:version-lockstep runs "
        "run.sh --self-test, --negative-control and bare, while this line is inside "
        "run_write(), reached only by `--write`. `--locked` would defeat the function, whose "
        "PURPOSE is to regenerate the lock after writing the six non-Cargo version sites. The "
        "scan is path-insensitive and cannot see this (L1), so the waiver stands in for it; "
        "check_version_lockstep_no_write below is what keeps the premise honest."
    ),
    ("ci/version-lockstep/run.sh", "cargo update -w >/dev/null )"): (
        "the un-offline fallback of the line above, same reason"
    ),
    (
        "ci/release-parity/ecosystems/release-plz.sh",
        'CARGO="$CARGO_BIN" CARGO_NET_OFFLINE=true "$RELEASE_PLZ_BIN" update 2>',
    ): (
        "MEASURED true positive, and correct as written (SMA-605). This is the arm-2 shape the "
        "whole change exists to see, and it is the only one in the repo: release-plz shells out "
        "to `cargo metadata`, and this line hands it an explicit CWD-independent cargo through "
        "the CARGO env var (SMA-596 D2.1). No --locked can reach that inner cargo — the flag "
        "would go to release-plz, which does not forward it, which is exactly why arm 2 carries "
        "wrapper semantics. The call is SAFE because it runs against a DISPOSABLE FIXTURE "
        "OUTSIDE the repo: ci/release-parity/run.sh:43 makes the dir with `mktemp -d` and :48 "
        "passes it to ecosystem::run_update, which cd's into it. So it cannot rewrite "
        "rs/Cargo.lock even though it resolves. If that fixture ever moves inside the workspace, "
        "DELETE THIS WAIVER — the reason no longer holds."
    ),
    ("ci/version-lockstep/run.sh", 'die_infra "cargo update -w failed (site 16)"'): (
        "PROSE, not an invocation: the failure message for the two lines above. The "
        "conservative rule does not strip quoted strings — that stripping is exactly what "
        "silently dropped real invocations before SMA-599's classifier was replaced — so a "
        "cargo verb inside a diagnostic surfaces as a row and is waived here instead. "
        "A false positive waived is the trade the design makes for never missing a real call."
    ),
    ("ci/actionlint/run.sh",
     'unlocked="${script/cargo metadata --locked --format-version 1/cargo metadata '
     '--format-version 1}"'): (
        "NOT an invocation: a `${var/old/new}` substitution that BUILDS the mutated script "
        "text for check 8f's own negative control, which then asserts that dropping --locked "
        "from ci/cargo-lock-integrity/run.sh is reported. Naming the unlocked form is the "
        "whole point of the line. It surfaced only once `_classify_shell_line` moved to "
        "finditer (SMA-599 review): the line holds TWO cargo invocations, the first carrying "
        "--locked, and reading just the first hid the second — the exact silent false "
        "negative the conservative rule claims it cannot have. One waiver is the price of "
        "making that claim true."
    ),
    ("ci/publish-metadata/run.sh",
     'die_infra "FATAL: \\`cargo metadata\\` failed in $RS_DIR — nothing could be verified."'): (
        "PROSE, same class as the entry above: the diagnostic for the `cargo metadata "
        "--no-deps` call on the joined logical line starting at :1663. The real invocation "
        "itself does not report, because --no-deps never resolves (MEASURED, §2.1)."
    ),
}

# The floor, for the reason REQUIRED_FFI_TASKS carries: a derived set that shrinks to EMPTY
# asserts nothing while still printing PASS. Every task named here MUST be in the derived set.
REQUIRED_LOCKED_TASKS = (
    "paigasus-kernel-rs:lint",
    "paigasus-iam-rs:test",
    "repo:deny",
    "repo:wasm-getrandom-free",
    # SMA-599 — these two reach cargo ONLY through a gate script, so they are the floor
    # members that fail if script-following silently stops working. Without them a broken
    # follower degrades the derived set in exactly the direction nothing else can see.
    # This landed in Task 3 rather than Task 2 DELIBERATELY: the floor is read by
    # check_cargo_locked, which cannot reach either task until this task's script arm
    # exists, so extending it earlier reds repo:affected-smoke on every commit in between.
    "repo:publish-metadata",
    "repo:version-lockstep",
)

# SMA-528 — the tasks that must key on their crate's upstream sources. `fmt` is crate-local by
# construction and `build-release` never runs in CI; neither carries `^:build` either.
UPSTREAM_INPUT_TASKS = ("build", "test", "lint")

# Consumer -> upstream pairs deliberately declared in `fileGroups.upstreams` WITHOUT being in the
# crate's Moon closure. A6 is strict-equality (SMA-429's default-deny model), so an intentional
# over-approximation needs an entry here with a non-empty reason. Empty today.
#
# MIND THE KEY SHAPE: it is (moon id, upstream SOURCE DIR) — e.g.
# ("paigasus-iam-rs", "rs/crates/libs/paigasus-kernel") — NOT the (moon id, moon id) shape
# ALLOW_NO_CARGO_BACKING uses twelve lines above. A6 recovers the upstream half STRUCTURALLY, from
# the first four `/`-separated segments of a resolved input path (`rs/crates/<layer>/<crate>`),
# not by stripping a known suffix — an over-approximating entry can carry any suffix now, not just
# `/src/**/*` or `/Cargo.toml`. A moon id never appears in a resolved path. A waiver written in the
# neighbouring shape is silently inert.
ALLOW_OVER_APPROXIMATION = {}

# A6's anti-vacuity floor, mirroring REQUIRED_FFI_TASKS. A6 DERIVES each crate's closure from
# moon's `dependencies` key; a moon rename or JSON reshape would empty every closure and A6 would
# print PASS for thirteen crates while asserting nothing. These edges must survive the derivation.
REQUIRED_CLOSURE_EDGES = {
    "paigasus-iam-rs": {"paigasus-kernel-rs", "paigasus-proto-rs"},
    "paigasus-kernel-parity-rs": {"paigasus-kernel-rs"},
}

# A7's anti-vacuity floor (SMA-560), and the reason it is EDGE-based rather than a task list.
# A7 asserts CONTAINMENT (`want <= observed`), and a containment check whose `want` empties is
# VACUOUSLY SATISFIED — it prints PASS having asserted nothing. A moon rename, a `dependencies`
# reshape or a `language` field change on a binding crate would each do that. A task-name floor
# cannot see it, because the tasks are still examined; only these edges can.
# The task SET needs no floor of its own: A7 derives it from derive_ffi_tasks(), whose own floor
# is REQUIRED_FFI_TASKS, already asserted by A5.
REQUIRED_WRAPPER_CLOSURE = {
    "paigasus-kernel-py": {"paigasus-kernel-rs", "paigasus-py-bindings-rs"},
    "paigasus-kernel-ts": {
        "paigasus-kernel-rs", "paigasus-node-bindings-rs", "paigasus-wasm-rs",
    },
}

# "moon reported no such task key at all", distinct from both None and []. A unique object, never a
# string: a string default flows into `set(declared or [])` and is iterated CHARACTER-WISE, turning
# a half-reported task into eight bogus single-letter entries instead of one honest violation.
_ABSENT = object()


def _allowlisted(allow, consumer, upstream):
    """True only if the entry exists AND carries a non-empty reason.

    Bare membership would let `("a", "b"): ""` silence A2 unreviewably, which defeats the point of
    the table: an allowlisted edge is a recorded decision, so the record is what earns the exemption.
    """
    return bool(allow.get((consumer, upstream), "").strip())


def check(projects, crates, allow=None):
    """Return (a1, a2, a3) violation lists.

    projects: {moon_id: {"source_dir": str,
                         "deps": {dep_id: "explicit"|"implicit"},
                         "tasks": {task_name: [resolved dep target, ...]}}}
    crates:   {crate_name: {"source_dir": str, "deps": {crate_name, ...}}}
    allow:    allowlist table; defaults to ALLOW_NO_CARGO_BACKING (injectable for the self-test).
    """
    allow = ALLOW_NO_CARGO_BACKING if allow is None else allow
    by_dir = {p["source_dir"]: mid for mid, p in projects.items()}
    a1, a2, a3 = [], [], []
    for _crate, info in sorted(crates.items()):
        mid = by_dir.get(info["source_dir"])
        if mid is None:
            continue
        want = {
            by_dir[crates[d]["source_dir"]]
            for d in info["deps"]
            if d in crates and crates[d]["source_dir"] in by_dir
        }
        have = projects[mid]["deps"]
        tasks = projects[mid]["tasks"]

        for upstream in sorted(want - set(have)):
            a1.append(f"{mid} -> {upstream}")
        for dep, src in sorted(have.items()):
            if (
                src == "explicit"
                and dep not in want
                and dep not in NON_CARGO_PARENTS
                and not _allowlisted(allow, mid, dep)
            ):
                a2.append(f"{mid} -> {dep}")
        # `lint` joined build/test in SMA-526: clippy propagates across a Moon edge only if the
        # task carries `^:build`, so a consumer's lint must schedule its upstreams' builds exactly
        # as its build and test do. Unlike build/test, lint's dep is declared ONCE for every crate
        # in .moon/tasks/rust.yml, so this row fires for all crates at once or not at all.
        for task in ("build", "test", "lint"):
            if want and tasks.get(task) is None:
                a3.append(f"{mid} has no `{task}` task (cannot schedule its upstream builds)")
        for upstream in sorted(want):
            for task in ("build", "test", "lint"):
                deps = tasks.get(task)
                if deps is not None and f"{upstream}:build" not in deps:
                    a3.append(f"{mid}:{task} does not schedule {upstream}:build")
    return a1, a2, a3


def check_task_inputs(projects, crates, task, required):
    """Return the A4 violation list: crates whose `task` does not key on `required`.

    A1-A3 are about dependency EDGES. A4 is about task INPUTS, and the two are independent: a crate
    can have a flawless edge set and still be structurally blind to a `rs/Cargo.lock` bump, because
    `rs/` has no Moon project for an edge to point at (SMA-534).

    Iterates EVERY crate unconditionally. It deliberately does not reuse `check()`'s `if want:`
    guard, which is only reached by crates that have in-tree dependencies: paigasus-kernel,
    paigasus-logging, paigasus-observability and paigasus-proto-derive have none, so copying that
    shape would leave four of thirteen unasserted with a green negative control.

    Spans BOTH input buckets (SMA-537). moon splits resolved inputs by kind: plain paths go to
    `inputFiles`, globs to `inputGlobs`. `lint`'s required set is all literals, but `fmt`'s is half
    globs (`@group(sources)`, `@group(tests)`), so a one-bucket read would silently assert nothing
    about them. An entry is matched if it appears in EITHER bucket verbatim; a crate-relative entry
    (`src/**/*`, `tests/**/*` — the group-derived globs) is also matched against the crate's OWN
    `source_dir`, because moon resolves those groups to exactly `<source_dir>/src/**/*` (SMA-560 M3).

    That anchoring replaced a `not f.startswith("rs/")` test plus an unanchored tail match, which
    was weak in two directions at once. A required entry outside `rs/` (`.prototools`, a
    `contracts/...` path) would have silently gained tail matching on the day it was added; and an
    unanchored tail let ANOTHER crate's glob satisfy this crate — `a-rs:fmt` counted as covered by
    `rs/crates/libs/b/src/**/*`. Anchoring drops the `rs/`-prefix special case entirely: a
    workspace-relative entry still has to match verbatim, since `<source_dir>/rs/rustfmt.toml` is
    not a path moon ever resolves.
    """
    by_dir = {p["source_dir"]: mid for mid, p in projects.items()}
    a4 = []
    for _crate, info in sorted(crates.items()):
        mid = by_dir.get(info["source_dir"])
        if mid is None:
            continue
        declared = projects[mid].get("task_inputs") or {}
        declared_globs = projects[mid].get("task_input_globs") or {}
        if task not in declared:
            a4.append(f"{mid} has no `{task}` task (nothing can key on {', '.join(required)})")
            continue
        files, globs = declared[task], declared_globs.get(task)
        if files is None or globs is None:
            a4.append(
                f"{mid}:{task} reported no `inputFiles`/`inputGlobs` — moon's output shape "
                f"changed, so this assertion cannot be evaluated (treated as a violation, "
                f"never skipped)"
            )
            continue
        observed = set(files) | set(globs)
        missing = []
        for f in required:
            if f in observed:
                continue
            # Crate-relative entries (`src/**/*`, `tests/**/*`) resolve to exactly
            # `<source_dir>/<entry>`, so ANCHOR the fallback to this crate's own source_dir rather
            # than accepting any tail match (SMA-560 M3). A workspace-relative entry (`rs/...`)
            # never matches this form, so it keeps its verbatim-only rule with no special case.
            if f"{info['source_dir']}/{f}" in observed:
                continue
            missing.append(f)
        if missing:
            a4.append(f"{mid}:{task} inputs omit {', '.join(missing)}")
    return a4


def derive_ffi_tasks(projects):
    """Every `<pid>:<task>` whose resolved invocation shells out to a Rust build.

    Shared by A5 (which asserts those tasks key on the workspace files) and A7 (which asserts the
    non-Rust ones key on their upstream crates' sources). Sharing it is deliberate: a wrapper that
    A5 covers and A7 does not — or the reverse — is a hole neither check can see.

    Raises MoonOutputError if a task exposes none of a command, a script, or any args.
    """
    matched = set()
    for pid in sorted(projects):
        invocations = projects[pid].get("invocations") or {}
        for name in sorted(invocations):
            blob = invocations[name]
            if blob is None:
                raise MoonOutputError(
                    f"{pid}:{name} reported none of a `command`, a `script`, or any `args` — "
                    f"moon's output shape changed, so the FFI derivation cannot be evaluated"
                )
            if any(marker in blob for marker in FFI_MARKERS):
                matched.add(f"{pid}:{name}")
    return matched


# Bracketed spans where a `<<` is a SHIFT and a `#` is a base marker, never a heredoc opener
# and never a comment: `$(( ... ))`, a bare `(( ... ))` arithmetic command, and anything in
# `[ ... ]` — an array subscript `a[1 << N]`, a `[[ ]]` test, a glob. The three are treated
# alike on purpose. A first cut required a word character before the `[`, to tell a subscript
# from the `[ -f x ]` test command; that guard changed no row across the corpus's 871
# non-subscript `[` occurrences and no fixture could distinguish it, because blanking here
# only ever REFUSES a cut or an open. An untestable guard is exactly what this gate keeps
# finding in its own code, so it is not shipped.
_ARITH_SPANS = (("$((", "(", ")"), ("((", "(", ")"), ("[", "[", "]"))


def _blank_operator_spans(masked):
    """Blank bracketed operator spans IN THE MASK ONLY, leaving the code text alone.

    Round 6 (SMA-599) turned this inside out, and the reason is the same one that merged the
    comment cut and the heredoc scan a round earlier. The old `_strip_arithmetic` ran on the
    RAW line, before any quote mask, and blanked from `$((` to EOL when the span never closed
    — so `echo '$(( x' && cargo build` blanked the real invocation out of the CODE and
    reported nothing. Blanking only the MASK cannot do that: the mask decides where a comment
    starts and whether a `<<` is an opener, and blanking there only ever REFUSES a cut or an
    open, which is the false-positive direction.

    Three consequences, all deliberate. A span inside a quoted string is already blanked by
    the quote mask, so it is never seen here. An UNCLOSED span is left alone entirely rather
    than swallowing the rest of the line. And the code region keeps every span verbatim, which
    is what makes the broad `[ ... ]` rule safe: `[ -n "$(cargo build)" ]` is blanked in the
    mask, so no `#` or `<<` inside it is trusted, while the invocation itself still classifies
    and still reports.
    """
    out, i, n = list(masked), 0, len(masked)
    while i < n:
        for prefix, opener, closer in _ARITH_SPANS:
            if not masked.startswith(prefix, i):
                continue
            depth, j = prefix.count(opener), i + len(prefix)
            while j < n:
                if masked[j] == opener:
                    depth += 1
                elif masked[j] == closer:
                    depth -= 1
                    if depth == 0:
                        break
                j += 1
            if j < n and depth == 0:
                out[i : j + 1] = " " * (j + 1 - i)
                i = j
            break
        i += 1
    return "".join(out)


def _escaped(text, idx):
    """True when `text[idx]` is preceded by an ODD number of backslashes, so it is quoted.

    `echo a\\ #b && cargo build` runs cargo: the backslash escapes the SPACE, so `#b` stays
    inside the word and starts no comment. Without this the word-start test sees a plain space
    before the `#`, cuts there, and the invocation disappears (MEASURED against bash).
    """
    n, j = 0, idx - 1
    while j >= 0 and text[j] == "\\":
        n += 1
        j -= 1
    return n % 2 == 1


# A real shell comment `#` starts a new WORD — it is preceded by whitespace, a shell
# metacharacter, or the start of the line. `${#arr[@]}` / `${#var}` (bash's length operator)
# put a `#` directly after `{`, mid-word, which is never a comment; `_line_regions` was
# cutting the line there and dropping everything after it, `n=${#arr[@]} && cargo build`
# included. Fixture and mutation both pin it.
_COMMENT_PRECEDING_CHARS = frozenset(" \t;&|()")


def _odd_quotes(masked, singles=False):
    """True when `masked` has an unpaired quote, so its quote state is AMBIGUOUS.

    `masked` has already had every matched pair blanked, so whatever is left is unpaired. An
    odd count means this physical line takes part in a quote span crossing physical lines and
    the mask therefore paired the wrong characters.

    `singles` is the difference between the two callers and it is deliberate, not an
    oversight. The heredoc decision counts BOTH quote characters, because opening a heredoc
    wrongly SWALLOWS every line up to the terminator — a silent pass. The comment cut counts
    DOUBLE quotes only, because an apostrophe in prose (`PKG_DIR's`, `don't`) is English, not
    shell quoting, and this repo's comments are full of them: counting singles there turns
    `ci/publish-metadata/run.sh:772` — a plain comment mentioning `cargo metadata` — into a
    would-report row (MEASURED). The residual that leaves open is L9, and it is narrow.
    """
    if masked.count('"') % 2:
        return True
    return bool(singles and masked.count("'") % 2)


def _line_regions(raw):
    """One PHYSICAL line's executable code region, and its heredoc opener (or None).

    Both of the conservative rule's line-local exclusions are decided here, from ONE
    within-line quote mask, because they are the same decision: which characters of this line
    does the shell actually execute. Splitting them is what let a `<<EOF` inside a string open
    a phantom heredoc and swallow a real invocation (SMA-599 round 5, measured against bash).

    The mask blanks matched quote pairs to EQUAL-LENGTH runs of spaces, so every surviving
    offset still indexes `raw`, and the returned code region is sliced out of the ORIGINAL.
    Nothing is removed but a comment TAIL: quotes, `$( ... )` and backticks all survive into
    what gets classified, because the conservative rule does not strip strings.

    Three decisions, each with its own fixture and its own mutation:

    1. **Ambiguous parity refuses BOTH.** An ODD count of surviving `"` or `'` means this line
       takes part in a quote span crossing physical lines, so the mask paired the wrong
       characters. Cut nothing and open nothing. Refusing to cut can only add a false
       positive; refusing to OPEN is likewise safe, because the would-be body is then scanned
       as ordinary code. Opening wrongly is what SWALLOWS, and swallowing is the silent-pass
       direction this design exists to avoid.
    2. **The `#` must start a word.** `${#arr[@]}` / `${#var}` (bash's length operator) put a
       `#` mid-word, where it is never a comment; cutting there drops the rest of the line,
       `n=${#arr[@]} && cargo build` included.
    3. **The `#` must be UNESCAPED.** `echo a\\ #b && cargo build` escapes the space, so `#b`
       is part of the word and not a comment marker (round 6).
    4. **A heredoc opener must be UNMASKED, and must not be a `<<<` here-string.**
       `HEREDOC_OPEN_RE` is matched against the code region but accepted only where the `<<`
       itself survives the mask, so `echo "a <<EOF b"` opens nothing while
       `cat <<'EOF' > "$out"` still does — there the `<<` sits outside every quote pair even
       though its delimiter is quoted. `cat <<<EOF` is a here-STRING: the regex matches at the
       second `<`, where the mask check passes, so the third `<` has to be rejected explicitly
       (round 6). The mask also carries `_blank_operator_spans`, which is what keeps a shift
       inside `$(( ))`, `(( ))` or `a[ ]` from reading as an opener.
    """
    quoted = SHELL_STRING_RE.sub(lambda m: " " * len(m.group(0)), raw)
    masked = _blank_operator_spans(quoted)
    cut = len(masked)
    # Parity is read off `quoted`, never `masked`: blanking an operator span could hide the
    # very unpaired quote that makes this line ambiguous.
    if not _odd_quotes(quoted):
        for idx, ch in enumerate(masked):
            if ch == "#" and (
                idx == 0
                or (
                    masked[idx - 1] in _COMMENT_PRECEDING_CHARS
                    and not _escaped(masked, idx - 1)
                )
            ):
                cut = idx
                break
    code, scan = raw[:cut], masked[:cut]
    if _odd_quotes(quoted[:cut], singles=True):
        return code, None
    for found in HEREDOC_OPEN_RE.finditer(code):
        at = found.start()
        if scan[at : at + 2] != "<<":
            continue
        if (at and code[at - 1] == "<") or code[at + 2 : at + 3] == "<":
            continue  # `<<<` is a here-STRING; it opens no body
        return code, found
    return code, None


def _join(pending):
    """One logical line from the `(lineno, text)` pairs in `pending` (continuations removed)."""
    return " ".join(text.rstrip().rstrip("\\") for _, text in pending)


def _tail_end(segment, start, hard_stop):
    """Where THIS invocation's flag scope ends.

    Bounded by whichever comes first: the next invocation (`hard_stop`), or the close of a
    command substitution this invocation sits INSIDE. Without the second bound a nested call
    inherits a flag that belongs to the enclosing one — measured on
    `cargo build --features "$(cargo test)" --locked`, where the nested `cargo test` read the
    outer `--locked` from its tail, was marked locked, and reported nothing. That is a SILENT
    FALSE NEGATIVE, the one failure this scanner exists to prevent (SMA-599, CodeRabbit round 1).

    A `)` at depth 0 closes an enclosing `$( )`; a backtick closes an enclosing legacy
    substitution. Either way the invocation ended there and later flags are not its own.
    """
    depth = 0
    for i in range(start, hard_stop):
        char = segment[i]
        if char == "(":
            depth += 1
        elif char == ")":
            if depth == 0:
                return i
            depth -= 1
        elif char == "`":
            return i
    return hard_stop


def cargo_matches(text):
    """Every cargo invocation in `text` — literal and indirect — sorted by start offset.

    `verb` is carried because the `--no-deps` carve-out must key on it. CARGO_METADATA_RE needs a
    literal lowercase `cargo`, so it never fires for `"$CARGO_BIN" metadata` and that call would
    report despite not resolving, contradicting SMA-599 D4.

    Arm 1's NAME FILTER RUNS HERE, before the list is merged. A rejected match left in the list
    would still act as a `stop` boundary in `_classify_shell_line` and truncate the PRECEDING
    invocation's tail — a silent false negative reached by the back door.

    DE-DUPLICATED ON END OFFSET, literal first. MEASURED: `$cargo build` already matches
    CARGO_INVOCATION_RE (`\\bcargo` needs only a word boundary, and `$` supplies one), so without
    this the lowercase form reports twice for one invocation and any waiver for it is permanently
    AMBIGUOUS — the SMA-599 L15 trap, reached by a new route.
    """
    found = [
        CargoMatch(m.start(), m.end(), m.group(0).split()[-1], "literal")
        for m in CARGO_INVOCATION_RE.finditer(text)
    ]
    found += [
        CargoMatch(m.start(), m.end(), m.group(2), "var")
        for m in CARGO_VAR_CMD_RE.finditer(text)
        if CARGO_VAR_NAME in m.group(1).lower()
    ]
    found += [
        CargoMatch(m.start(), m.end(), None, "env")
        for m in CARGO_ENV_PREFIX_RE.finditer(text)
    ]
    # KIND FIRST, then position. Sorting by position first is WRONG and was measured: arm 1's
    # match on `$cargo build` starts at the `$` (offset 0) while the literal match starts at the
    # `c` (offset 1), so a position-first sort lets `var` claim the shared end offset and the
    # invocation reports as indirect. The kinds must not be interchangeable here for the same
    # reason check_cargo_locked keeps them apart.
    rank = {"literal": 0, "var": 1, "env": 2}
    out, claimed = [], set()
    for match in sorted(found, key=lambda c: (rank[c.kind], c.start)):
        if match.end in claimed:
            continue
        claimed.add(match.end)
        out.append(match)
    # An env prefix whose COMMAND is cargo itself is not indirection, and reporting both is worse
    # than redundant: the two rows carry the SAME segment text, so every waiver for that line is
    # permanently AMBIGUOUS and the line cannot be waived at all — SMA-599 L15, reached by a new
    # route. MEASURED on `CARGO=/p cargo build --locked`, a correctly locked call that reported
    # twice and could not be cleared (CodeRabbit PR review). The gap may hold further `NAME=value`
    # assignments, which is why it is not a bare adjacency test.
    real = [c for c in out if c.kind in ("literal", "var")]
    out = [
        c
        for c in out
        if c.kind != "env"
        or not any(
            m.start >= c.end and ENV_ONLY_GAP_RE.match(text[c.end : m.start]) for m in real
        )
    ]
    return sorted(out, key=lambda c: c.start)


def _classify_shell_line(lineno, logical):
    """Rows for one LOGICAL line (backslash continuations already joined).

    The conservative rule, in full: split the line on `;`, `&` and `|`, and emit a row for
    EVERY cargo invocation in every resulting segment. Nothing else is consulted — no string
    stripping, no exec detection, no substitution extraction.

    ONE ROW PER INVOCATION, not per segment (SMA-599 review). `finditer`, not `search`:
    `cargo build --locked --features "$(cargo test)"` is one segment holding two invocations,
    and reading only the first reported a single locked row while the nested `cargo test`
    vanished — a SILENT FALSE NEGATIVE, the one failure direction this design claims it
    cannot have. A live instance of the shape sits at `ci/actionlint/run.sh:3715` (benign; it
    is a `${var/old/new}` substitution naming both the locked and the unlocked form), and it
    is waived in ALLOW_UNLOCKED_CARGO_SCRIPT.

    Each invocation's flags are read from ITS OWN TAIL: the text from the end of its verb to
    the start of the NEXT invocation in the same segment, or to the end of the segment. That
    bound is what stops the nested call's flag from covering the outer one —
    `cargo build "$(cargo test --locked)"` reports `cargo build`, because the `--locked`
    belongs to the invocation after it. The segment scope is what keeps
    `cargo build && cargo metadata --locked` reporting `cargo build`; the after-the-verb scope
    is what stops a `--locked` that is string content sitting BEFORE the verb from covering a
    genuinely unlocked call — `X="abc` newline `--locked" cargo build` is one bash statement
    across two physical lines, and the second line reaches cargo unlocked.

    Every row from one segment carries that WHOLE segment as its `segment` text, deliberately:
    it is the waiver key, and an invocation's own span alone (`cargo metadata\\` failed …`)
    reads as garbage to the reviewer who has to judge the waiver. Two rows sharing a text is
    handled where the waivers are checked, not here.
    """
    rows = []
    for segment in COMMAND_SPLIT_RE.split(logical):
        found = cargo_matches(segment)
        for idx, match in enumerate(found):
            stop = found[idx + 1].start if idx + 1 < len(found) else len(segment)
            tail = segment[match.end : _tail_end(segment, match.end, stop)]
            # MEASURED (SMA-599 §2.1): `cargo metadata --no-deps` does not resolve and never
            # rewrites the lock, so --locked on it is INERT. Demanding the flag would be
            # cargo-cult compliance a later reader would mistake for a guarantee. Read from
            # THIS invocation's verb and tail, never the whole segment: a `--no-deps` on a
            # neighbouring call must not excuse a resolving one.
            # Keyed on the matched VERB, not on CARGO_METADATA_RE over the match text: that
            # regex needs a literal lowercase `cargo`, so an arm-1 `"$CARGO_BIN" metadata
            # --no-deps` would report despite not resolving, contradicting D4 (SMA-605 review).
            resolves = not (match.verb == "metadata" and re.search(r"--no-deps\b", tail))
            rows.append(
                ScriptCargoLine(
                    lineno, logical, segment, resolves, LOCKED_FLAG in tail, match.kind
                )
            )
    return rows


def script_cargo_lines(path):
    """Every cargo invocation in a shell script, with its flags classified.

    Raises MoonOutputError on a heredoc still open at EOF: otherwise the scanner silently
    skips the rest of the file and reports zero rows, which is a vacuous PASS by a different
    route. Same "infrastructure, never a silent pass" contract check_dockerfile_locked uses.
    A quoted string open at EOF raises nothing any more — with no string stripping there is
    no quote state to get wrong, so such a file classifies as ordinary code.
    """
    rows, delim, held, pending = [], None, None, []
    for lineno, raw in enumerate(Path(path).read_text().splitlines(), 1):
        if delim is not None:
            if raw.strip() == delim:
                delim = None
            continue
        # `_line_regions` runs PER PHYSICAL LINE and BEFORE the continuation test, not after
        # joining. A `#` comment ends at the newline even when the line ends in a backslash
        # (a backslash is not special inside a comment), so joining first would pull the next
        # line's real invocation into the comment and drop it. It also strips `$(( ... ))`
        # first, because `HEREDOC_OPEN_RE` matches a bare `<<` and `$((1 << BITS))` would
        # otherwise read as a heredoc named BITS.
        work, opener = _line_regions(raw)
        pending.append((lineno, work))
        if opener is not None and held is None:
            held = opener.group(2)
        # A heredoc body starts after the whole LOGICAL line, not after the physical line
        # carrying the opener: `cat <<EOF \\` + newline + `| cargo build` is one command, and
        # treating the continuation as body swallowed it (round 6, MEASURED against bash). So
        # the opener is HELD across the continuation instead of ending it.
        if work.rstrip().endswith("\\"):
            continue
        rows.extend(_classify_shell_line(pending[0][0], _join(pending)))
        pending = []
        delim, held = held, None
    if pending:
        rows.extend(_classify_shell_line(pending[0][0], _join(pending)))
    if delim is not None:
        raise MoonOutputError(
            f"{path}: heredoc `{delim}` is still open at EOF — the scan would silently skip "
            f"the rest of the file and report zero rows"
        )
    return rows


def task_script_refs(projects, root, target):
    """The `ci/**/*.sh` files a task's resolved invocation runs, as existing Paths.

    Raises MoonOutputError when a referenced script does not resolve to a readable file: a
    rename would otherwise silently empty the derived set.
    """
    pid, _, name = target.partition(":")
    blob = (projects[pid].get("invocations") or {}).get(name)
    if not blob:
        return []
    paths = []
    for rel in sorted(set(SCRIPT_REF_RE.findall(blob))):
        path = Path(root) / rel
        if not path.is_file():
            raise MoonOutputError(
                f"{target} invokes {rel}, which does not resolve to a readable file — the "
                f"derived set would silently shrink. If the script moved, update the task."
            )
        paths.append(path)
    return paths


# SMA-605 - SMA-599's L2, `source` half closed TRANSITIVELY (cycle-guarded). EXECUTION ONLY.
#
# A `source` / `.` statement is followed. A bare `ci/**/*.sh` mention in a script's text is NOT:
# running SCRIPT_REF_RE over a followed script's own text was MEASURED at six new edges across
# the real corpus, and every one of them is a comment or a pin-array string constant
# (publish-metadata's :9-10, :1686, :1726; actionlint's :2016, :2041, :2046 and its
# T_CARGO_LOCK_STEP_REQUIRED array at :2152-2154). That buys six scripts into A8's scope on the
# strength of prose, one new waiver, and ZERO true positives - and it still does not reach
# ci/release-parity/ecosystems/release-plz.sh, because SCRIPT_REF_RE cannot match a
# `# shellcheck source=...` directive (the path is preceded by `=`, not by a separator).
SOURCE_STMT_RE = re.compile(r"""(?m)^\s*(?:source|\.)\s+["']?([^"'\s;&|]+)["']?""")
# A variable assigned from the `dirname "${BASH_SOURCE[0]}"` idiom IS the script's own directory.
# VAR_ASSIGN_RE cannot capture it - it stops at the space inside `$(cd ...` - so this reads the
# raw assignment line instead.
HERE_IDIOM_ASSIGN_RE = re.compile(r"""(?m)^\s*([A-Za-z_][A-Za-z0-9_]*)="?\$\(cd .*BASH_SOURCE.*""")

# The resolver's floor. Without it a rename empties the closure in silence - the SMA-553 class.
REQUIRED_SOURCED_SCRIPTS = {
    "ci/release-parity/run.sh": (
        "ci/release-parity/ecosystems/python-semantic-release.sh",
        "ci/release-parity/ecosystems/release-plz.sh",
        "ci/release-parity/ecosystems/semantic-release.sh",
    ),
}


def _executable_text(path):
    """`path`'s text with heredoc BODIES and comment tails removed.

    `script_cargo_lines` already refuses to classify a heredoc body, for the reason that the
    shell never executes it. `script_source_refs` claimed to be EXECUTION ONLY while scanning
    RAW text, so this valid script aborted the whole gate:

        cat <<'EOF'
        source ./missing.sh
        EOF

    `SOURCE_STMT_RE` matched the body line and the resolver raised MoonOutputError on the absent
    target — an infrastructure failure on a benign file (MEASURED, CodeRabbit PR review). Reusing
    `_line_regions` and the same heredoc walk keeps the two scanners' notion of "executed" in one
    place instead of two that can drift.

    Raises MoonOutputError on a heredoc still open at EOF, the contract `script_cargo_lines` uses:
    otherwise the rest of the file is skipped in silence.
    """
    out, delim, held = [], None, None
    for raw in Path(path).read_text().splitlines():
        if delim is not None:
            if raw.strip() == delim:
                delim = None
            continue
        work, opener = _line_regions(raw)
        out.append(work)
        if opener is not None and held is None:
            held = opener.group(2)
        if work.rstrip().endswith("\\"):
            continue
        delim, held = held, None
    if delim is not None:
        raise MoonOutputError(
            f"{path}: heredoc `{delim}` is still open at EOF — the source scan would silently "
            f"skip the rest of the file"
        )
    return "\n".join(out)


def script_source_refs(path, root):
    """The scripts `path` EXECUTES through a `source` / `.` statement.

    A variable assigned EXACTLY ONCE resolves to its value; one assigned more than once - or
    never - becomes a glob. MEASURED on the one source statement in the tree
    (ci/release-parity/run.sh:21): HERE is assigned once, at :7, so it resolves to the script's
    directory, while ECOSYSTEM is assigned at :8 AND :13 (from `$2`), so it globs and yields all
    three ecosystem modules rather than only the default release-plz. The other two are real code
    a Moon task executes, and a resolver returning only the default would leave them unscanned.

    Over-approximation in the same direction as the path-insensitive scan it feeds (SMA-599 L1):
    all three modules land in every release-parity* task's closure, including the two a given
    invocation never sources.

    Raises MoonOutputError when a source resolves to nothing - a rename would otherwise shrink
    the closure in silence, which is the failure this whole change exists to prevent.
    """
    # RESOLVED at entry, deliberately: `$HERE` expands to `str(path.parent)`, so a relative
    # `path` produced a relative expansion, the `not candidate.is_absolute()` branch prepended
    # the parent a SECOND time, and every source resolved to nothing (MEASURED — it raised
    # rather than passing quietly, but the trap is real and one line removes it).
    path = Path(path).resolve()
    # EXECUTABLE text, never raw: a `source` inside a heredoc body is not executed, and treating
    # it as one aborted the gate (see _executable_text).
    text = _executable_text(path)
    counts = collections.Counter(name for name, _ in VAR_ASSIGN_RE.findall(text))
    env = {name: value for name, value in VAR_ASSIGN_RE.findall(text) if counts[name] == 1}
    for name in HERE_IDIOM_ASSIGN_RE.findall(text):
        if counts[name] <= 1:
            env[name] = str(path.parent)
    # Longest name first, for the reason _cwd_inside_rs records: `str.replace` on the bare $NAME
    # form has no word boundary, so a short name that prefixes a longer one eats it.
    ordered = sorted(env.items(), key=lambda kv: (-len(kv[0]), kv[0]))
    root_resolved = Path(root).resolve()
    out = []
    for raw in SOURCE_STMT_RE.findall(text):
        target = raw
        for name, value in ordered:
            target = target.replace("${" + name + "}", value).replace("$" + name, value)
        # The `$(dirname ...)` form used inline rather than through a variable.
        target = re.sub(r"\$\(dirname [^)]*\)", str(path.parent), target)
        # Anything still unresolved becomes a glob.
        target = re.sub(r"\$\{?[A-Za-z_][A-Za-z0-9_]*\}?", "*", target)
        candidate = Path(target)
        if not candidate.is_absolute():
            candidate = path.parent / candidate
        hits = sorted(Path(candidate.anchor or "/").glob(str(candidate).lstrip("/")))
        # Files only, and never outside the repo: a `source /etc/profile` is not a gate script,
        # and scanning one would put text nobody reviews into A8's corpus.
        hits = [h for h in hits if h.is_file() and root_resolved in h.resolve().parents]
        if not hits:
            raise MoonOutputError(
                f"{path}: `source {raw}` resolves to no readable file inside the repo - the "
                f"script closure would silently shrink. If the module moved, update the source "
                f"statement."
            )
        out.extend(hits)
    return out


def task_script_closure(projects, root, target):
    """`task_script_refs` plus the transitive `source` closure, cycle-guarded.

    Breadth-first with a visited set keyed on the RESOLVED path, so a cycle terminates and a
    module reached twice appears once. Depth is unbounded by design; the corpus is depth 2.

    Every member is returned RESOLVED, and that is load-bearing. `task_script_refs` builds
    `root / rel`, which keeps whatever form the caller passed, while `script_source_refs`
    resolves. Mixing the two crashed a consumer: with a symlinked `root`, the closure held one
    path in link form and one in real form, and `check_cargo_locked_scripts`'
    `path.relative_to(root)` raised ValueError — which is NOT in INFRA_ERRORS, so it escaped as a
    traceback instead of the rc-2 infrastructure classification (MEASURED, CodeRabbit PR review).
    Consumers must therefore compare against a RESOLVED root.
    """
    queue, seen, out = list(task_script_refs(projects, root, target)), set(), []
    while queue:
        key = queue.pop(0).resolve()
        if key in seen:
            continue
        seen.add(key)
        out.append(key)
        queue.extend(script_source_refs(key, root))
    return out


def check_sourced_scripts(root, required=None):
    """REQUIRED_SOURCED_SCRIPTS, asserted. Rows join A8's bucket in collect_findings."""
    required = REQUIRED_SOURCED_SCRIPTS if required is None else required
    root_resolved = Path(root).resolve()
    rows = []
    for rel, expected in sorted(required.items()):
        path = root_resolved / rel
        if not path.is_file():
            rows.append(f"{rel} is absent - the source resolver's floor cannot be evaluated")
            continue
        got = tuple(sorted(
            x.resolve().relative_to(root_resolved).as_posix()
            for x in script_source_refs(path, root_resolved)
        ))
        if got != tuple(sorted(expected)):
            rows.append(
                f"{rel} sources {got}, expected {tuple(sorted(expected))} - the source resolver "
                f"has degraded and the script closure would silently shrink"
            )
    return rows



def derive_cargo_tasks(projects, root):
    """{target: kind} for every task reaching cargo. kind is wrapper | literal | script.

    NOT a flat set, deliberately. check_cargo_locked records, as measured, that a wrapper
    match and a literal match must not be treated alike: `paigasus-kernel-ts:build` runs an
    unlocked `napi build` beside a `wasm-pack build ... -- --locked`, so a blob-level flag
    test greens a task that still repairs the lock. Collapsing the kinds here would
    reintroduce that measured-vacuous form one level down.

    Precedence is wrapper > literal > script, matching check_cargo_locked's existing rule
    that a task matching both kinds is governed by the stricter (wrapper) one.
    """
    kinds = {}
    for pid in sorted(projects):
        invocations = projects[pid].get("invocations") or {}
        for name in sorted(invocations):
            target, blob = f"{pid}:{name}", invocations[name]
            if blob is None:
                continue
            # Arm 2 folds into the WRAPPER kind rather than becoming a fourth one: `CARGO=<path>
            # <tool>` reaches cargo through a tool that takes no --locked, which is exactly the
            # FFI wrapper contract, and reusing it means the existing ALLOW_UNLOCKED_CARGO
            # semantics apply unchanged (SMA-605 §5.4).
            # MERGED matches, never the raw env regex: `cargo_matches` already drops an env
            # prefix whose command IS cargo, so `CARGO=/p cargo build --locked` is one locked
            # literal call. Searching CARGO_ENV_PREFIX_RE directly bypassed that and classified
            # the same correct line as a wrapper needing a waiver — the blob arm and the script
            # arm disagreeing about one string (MEASURED, CodeRabbit PR review).
            blob_matches = cargo_matches(blob)
            if any(marker in blob for marker in FFI_MARKERS) or any(
                c.kind == "env" for c in blob_matches
            ):
                kinds[target] = "wrapper"
            elif any(c.kind in ("literal", "var") for c in blob_matches):
                kinds[target] = "literal"
            elif any(
                script_cargo_lines(p) for p in task_script_closure(projects, root, target)
            ):
                kinds[target] = "script"
    return kinds


def check_cargo_locked(projects, root=None, allow=ALLOW_UNLOCKED_CARGO, floor=REQUIRED_LOCKED_TASKS):
    """Return the A8 violation list: cargo-resolving tasks that do not pass --locked.

    An unlocked cargo invocation re-resolves the graph and REWRITES an inconsistent Cargo.lock in
    place. That is how five Dependabot PRs merged a truncated lock through a green `moon ci`: the
    first cargo task repaired the lock, and every later task read a resolution the PR never
    shipped.

    Two match kinds, deliberately NOT treated alike (SMA-601):

    * A literal `cargo <verb>` match (CARGO_INVOCATION_RE) is satisfied by `--locked` in the blob.
      The blob IS the invocation there, so the flag reaching it is the whole assertion.
    * An FFI_MARKERS match is a WRAPPER, and its own cargo call takes no flag. Such a task always
      needs an `allow` entry, whether or not `--locked` appears in its blob — measured, because
      `paigasus-kernel-ts:build` runs an unlocked `napi build` beside a
      `wasm-pack build ... -- --locked`, so a blob-level flag test greens a task that still
      repairs the lock. A task matching BOTH kinds is governed by the wrapper rule.

    An `allow` entry is a RECORDED DECISION: an empty reason is itself a violation, the idiom
    ALLOW_NO_CARGO_BACKING and ALLOW_DEAD_INPUT already use.

    The FLOOR carries REQUIRED_FFI_TASKS' job: a derived set that degrades to empty asserts
    nothing while still printing PASS.

    Raises MoonOutputError if a task in `floor` exposes none of a command, a script, or any
    args — the same absent-invocation contract A5 uses. That contract is WEAKER than
    `derive_ffi_tasks`', which raises on ANY None blob: a None outside `floor` is skipped here.
    The difference is unreachable today only because `collect_findings` computes `a5` before `a8`,
    so A5's stricter rule aborts the run first. Reordering or removing A5 makes it reachable.

    SMA-599 — `root`, when given, widens the FLOOR match (never the row emission above) with
    `derive_cargo_tasks(projects, root)`. A task reaching cargo ONLY through a gate script
    (`repo:publish-metadata`, `repo:version-lockstep`) never contains a literal `cargo <verb>`
    or an FFI_MARKERS string in its OWN blob, so this function's blob-only `matched` can never
    see it — that gap is exactly why those two floor members had to wait for the script arm
    (`check_cargo_locked_scripts`) to exist before joining REQUIRED_LOCKED_TASKS. Widening only
    the floor check, not row emission, keeps this function from reporting a script's own
    unlocked lines twice — `check_cargo_locked_scripts` is the sole source of those rows.
    `root=None` (every self-test call) preserves the old blob-only floor exactly.
    """
    rows = []
    matched = set()
    for pid in sorted(projects):
        invocations = projects[pid].get("invocations") or {}
        for name in sorted(invocations):
            target = f"{pid}:{name}"
            blob = invocations[name]
            if blob is None:
                if target in floor:
                    raise MoonOutputError(
                        f"{target} reported none of a `command`, a `script`, or any `args` — "
                        f"moon's output shape changed, so A8 cannot be evaluated"
                    )
                continue
            blob_matches = cargo_matches(blob)
            is_ffi = any(marker in blob for marker in FFI_MARKERS)
            is_wrapper = bool(is_ffi or any(c.kind == "env" for c in blob_matches))
            # Name the CAUSE. `is_wrapper` covers two shapes since SMA-605, and a row reading
            # "(FFI_MARKERS)" for a `CARGO=` blob sends the reviewer hunting for a napi /
            # wasm-pack / maturin call that is not there. The script arm already distinguishes
            # them; this is the one place that did not (CodeRabbit PR review).
            cause = "a wrapper (FFI_MARKERS)" if is_ffi else "a CARGO= redirection"
            if not (is_wrapper or any(c.kind in ("literal", "var") for c in blob_matches)):
                continue
            matched.add(target)
            if not is_wrapper and LOCKED_FLAG in blob:
                continue
            reason = allow.get(target)
            if reason is None:
                if is_wrapper:
                    rows.append(
                        f"{target} reaches cargo through {cause}, whose own "
                        f"cargo call cannot take {LOCKED_FLAG} — a {LOCKED_FLAG} elsewhere in "
                        f"the script does NOT cover it, so this task needs an "
                        f"ALLOW_UNLOCKED_CARGO entry: {blob[:120]}"
                    )
                else:
                    rows.append(
                        f"{target} reaches cargo without {LOCKED_FLAG} — it will re-resolve and "
                        f"REWRITE an inconsistent Cargo.lock in place: {blob[:120]}"
                    )
            elif not reason.strip():
                rows.append(
                    f"{target} is in ALLOW_UNLOCKED_CARGO with an empty reason — an exemption "
                    f"is allowed, a silent one is not"
                )
    # SMA-599 — widen the FLOOR match only (see docstring); row emission above is unaffected.
    floor_matched = matched | set(derive_cargo_tasks(projects, root)) if root is not None else matched
    for target in sorted(set(floor) - floor_matched):
        rows.append(
            f"A8 examines {len(matched)} task(s) and {target} is not among them — the "
            f"derivation has degraded and would assert nothing"
        )
    return rows


def _row_reports(line):
    """Whether this row is a violation. ONE definition, used by BOTH loops below.

    Named `_row_reports`, not `_reports`: `self_test` already has a LOCAL helper called
    `_reports`, and a nested `def` makes that name local for the whole enclosing function, so a
    module-level `_reports` is unreachable from every fixture (UnboundLocalError, measured).

    An `env` row is NEVER satisfied by a flag: `CARGO=<path> <tool>` reaches cargo through the
    tool, and the tool takes no `--locked`. Reading `line.locked` for it lets the TOOL's own flag
    clear the row, because that flag lands inside arm 2's tail.

    Both loops must share this. With emission kind-aware and the waiver-health loop kind-blind,
    an `env` row whose tool carries `--locked` is emitted, the reviewer adds a waiver, emission
    clears, and the health loop then finds no hits and reports the honest waiver as STALE. The
    row is permanently red with no escape but rewriting the shell line (SMA-605 review).
    """
    return line.kind == "env" or (line.resolves and not line.locked)


def check_cargo_locked_scripts(projects, root, allow=None):
    """A8 rows for cargo invocations inside the gate scripts a Moon task runs.

    A blob-level derivation cannot see these: `repo:publish-metadata`'s invocation is
    `bash ci/publish-metadata/run.sh`, while its `cargo package --list --locked` and
    `cargo publish --dry-run --locked` live in the script. Before SMA-599 that whole class of
    gate was outside A8.

    Path-INSENSITIVE (SMA-599 L1): it reports a line the task's arguments may never reach.
    That is why the version-lockstep waiver exists, and why a reviewer must check reachability
    by hand rather than trusting a row.
    """
    allow = ALLOW_UNLOCKED_CARGO_SCRIPT if allow is None else allow
    # RESOLVED, to match task_script_closure's members. An unresolved root against a resolved
    # member raises ValueError out of relative_to, and ValueError is not in INFRA_ERRORS.
    root_resolved = Path(root).resolve()
    rows, seen = [], {}
    for target in sorted(derive_cargo_tasks(projects, root)):
        for path in task_script_closure(projects, root, target):
            rel = path.relative_to(root_resolved).as_posix()
            if rel in seen:
                continue
            lines = script_cargo_lines(path)
            seen[rel] = lines
            for line in lines:
                text = line.segment.strip()
                # Task 1's conservative rule leaves nothing "unclassifiable": every row is an
                # ordinary row, and a benign string that mentions a cargo verb is waived here.
                # Use `line.locked`, not `LOCKED_FLAG in text` — the classifier already scoped
                # the flag to the segment tail AFTER the verb, and a bare substring test on the
                # segment throws that scoping away.
                if not _row_reports(line):
                    continue
                reason = allow.get((rel, text))
                if reason is None:
                    if line.kind == "env":
                        rows.append(
                            f"{rel}:{line.lineno} sets CARGO= to redirect cargo through another "
                            f"tool, which cannot take {LOCKED_FLAG} — a {LOCKED_FLAG} on the "
                            f"tool does NOT cover it, so this line needs an "
                            f"ALLOW_UNLOCKED_CARGO_SCRIPT entry: {text}"
                        )
                    else:
                        rows.append(
                            f"{rel}:{line.lineno} reaches cargo without {LOCKED_FLAG} — it will "
                            f"re-resolve and REWRITE an inconsistent Cargo.lock in place: "
                            f"{text}"
                        )
                elif not reason.strip():
                    rows.append(
                        f"{rel}:{line.lineno} is in ALLOW_UNLOCKED_CARGO_SCRIPT with an empty "
                        f"reason — an exemption is allowed, a silent one is not"
                    )
    # Stale and ambiguous waiver entries. A waiver that matches nothing has silently stopped
    # asserting; one that matches twice covers a line nobody reviewed.
    #
    # Only REPORTING rows are counted (SMA-599 review). Since `_classify_shell_line` emits one
    # row per invocation rather than per segment, a segment holding two invocations yields two
    # rows carrying the same segment text — `ci/actionlint/run.sh:3715` is the live instance,
    # where the first invocation is locked and the second is not. A row that does not report
    # needs no waiver, so a waiver cannot be "covering a line nobody reviewed" by matching it;
    # counting it would red an honest waiver. A waiver whose line stops reporting still reads
    # as STALE, which is the correct verdict — the exemption is no longer earning its place.
    for (rel, text), _reason in sorted(allow.items()):
        hits = [
            line
            for line in seen.get(rel, [])
            if line.segment.strip() == text and _row_reports(line)
        ]
        if not hits:
            rows.append(
                f"ALLOW_UNLOCKED_CARGO_SCRIPT entry ({rel}, {text[:60]!r}) matches no line — "
                f"the waiver is stale; delete it or update the text"
            )
        elif len(hits) > 1:
            rows.append(
                f"ALLOW_UNLOCKED_CARGO_SCRIPT entry ({rel}, {text[:60]!r}) occurs "
                f"{len(hits)} times — the key is ambiguous and would waive a line nobody reviewed"
            )
    return rows


def check_version_lockstep_no_write(projects):
    """The premise of ALLOW_UNLOCKED_CARGO_SCRIPT's two entries, asserted.

    Their reason is "unreachable, because the Moon task never passes --write". Adding
    `--write` to that task would make both waivers silently wrong, so assert it directly.
    """
    blob = (projects.get("repo", {}).get("invocations") or {}).get("version-lockstep")
    if blob is None:
        return [
            "repo:version-lockstep has no resolved invocation — ALLOW_UNLOCKED_CARGO_SCRIPT's "
            "reachability premise cannot be evaluated"
        ]
    if "--write" in blob:
        return [
            "repo:version-lockstep now passes --write, so its `cargo update -w` lines ARE "
            "reachable and their ALLOW_UNLOCKED_CARGO_SCRIPT waivers are wrong (SMA-599 §2.4)"
        ]
    return []


def _cwd_inside_rs(text, source_dir):
    """True when this task's cargo runs with cwd under `rs/`.

    Reads RAW text, never the quote-stripped form: stripping strings first turns
    `RS_DIR="$REPO_ROOT/rs"` into `RS_DIR=` and `cd "$RS_DIR"` into `cd`, and the whole
    shape dies.

    Substitution runs LONGEST NAME FIRST (SMA-599 review, MEASURED false negative before this
    fix). `str.replace` on the bare `$NAME` form has no word boundary, so a short name that
    prefixes a longer one eats it: with `R=zzz` and `RS_DIR="$REPO_ROOT/rs"`, substituting `R`
    first turned `$RS_DIR` into `zzzS_DIR` and `cd "$RS_DIR"` resolved to False. Real followed
    scripts do carry prefix collisions, and dict order is the file's assignment order, so the
    outcome depended on which variable a script happened to define first. Longest-first removes
    the class: a longer name is always tried before any of its prefixes.
    """
    if source_dir == "rs" or source_dir.startswith("rs/"):
        return True
    env = dict(VAR_ASSIGN_RE.findall(text))
    ordered = sorted(env.items(), key=lambda kv: (-len(kv[0]), kv[0]))
    # Scan the text with substitutions REMOVED and each substitution's BODY, because a cd can
    # live on either side and the two need opposite treatment (SMA-599, CodeRabbit round 2).
    #   cd "$(git rev-parse --show-toplevel)/rs"   -> only the stripped form works: with the
    #       substitution left in, CWD_TOKEN_RE stops at the space inside it and captures `"$(git`.
    #   X="$(cd rs && cargo build)"                -> only the BODY works: stripping deletes the
    #       `cd rs` outright, and the task then reads as running outside rs/ — a silent false
    #       negative, which is the failure this assertion exists to prevent.
    # Scanning both is what covers the pair; scanning either alone drops the other.
    chunks = [CMD_SUBST_RE.sub("", text), *CMD_SUBST_RE.findall(text)]
    for token in [tok for chunk in chunks for tok in CWD_TOKEN_RE.findall(chunk)]:
        resolved = token
        for name, value in ordered:
            resolved = resolved.replace(f"${{{name}}}", value).replace(f"${name}", value)
        if RS_PATH_RE.search(resolved):
            return True
    return False


def _var_sensitive(text):
    """True when a cargo-NAMED variable runs a COMPILING subcommand in `text`."""
    return any(
        CARGO_VAR_NAME in m.group(1).lower()
        for m in CARGO_VAR_CMD_SENSITIVE_RE.finditer(text)
    )


def check_cargo_config_inputs(projects, root, allow=None, floor=None):
    """A10: every task whose cargo can READ rs/.cargo/config.toml must key on it.

    Scope is the conjunction of two independent tests, and both matter:
      * the subcommand is in CONFIG_SENSITIVE_VERBS (it compiles or links); and
      * cwd resolves inside `rs/` (cargo finds the file by walking UP from cwd).

    Spans both input buckets and treats an absent bucket as a violation, never a skip — the
    contract A4/A5/A6/A7 share. MEASURED: `moon.yml:239`'s `rs/.cargo/config.toml` and
    `.moon/tasks/rust.yml:46`'s `/rs/.cargo/config.toml` both resolve to the same slash-free
    path, which is why all 58 declaring tasks match verbatim.

    Whenever a task's invocation names a `ci/**/*.sh`, CONFIG_SENSITIVE_RE and CWD_TOKEN_RE run
    over that file's RAW text — for EVERY kind, not only `script` — comments, heredoc bodies
    and string literals included, unlike A8's region-stripped scan. This is over-approximation
    ONLY: it can only pull a task INTO scope that a stricter scan would have left out (a false
    "in scope", never a false "out of scope"), so the failure direction is a row a human must
    dismiss by hand, not a silently missed one (SMA-599 review).
    """
    allow = ALLOW_MISSING_CARGO_CONFIG if allow is None else allow
    floor = REQUIRED_CARGO_CONFIG_TASKS if floor is None else floor
    rows, in_scope = [], set()
    for target, kind in sorted(derive_cargo_tasks(projects, root).items()):
        pid, _, name = target.partition(":")
        blob = projects[pid]["invocations"][name]
        text = blob
        # Followed for EVERY kind, not only `script` (SMA-599 review). `derive_cargo_tasks`
        # assigns `literal` on any cargo verb ANYWHERE in the blob, prose included, so a gate
        # whose blob reads `echo "running cargo check"; bash ci/foo/run.sh` is `literal` while
        # the identical gate without the echo is `script` — and the guard then read the script
        # for the second and not the first. A benign `echo` in a moon.yml block silently
        # switched A10 off for that gate. `task_script_refs` returns [] when a blob names no
        # script, so this costs nothing for a task that really is blob-only: MEASURED on the
        # real corpus, in_scope stays 58 and no row appears.
        for path in task_script_closure(projects, root, target):
            text += "\n" + Path(path).read_text()
        # A wrapper reaches cargo without a literal verb, so the verb test cannot see it. The
        # three FFI tasks compile and link cdylibs and wasm32 by construction.
        # Arm 2 makes a task sensitive UNCONDITIONALLY: `CARGO=<path> <tool>` reaches cargo
        # through a tool whose subcommand A10 cannot read, so it cannot rule out a compile.
        sensitive = (
            kind == "wrapper"
            or bool(CONFIG_SENSITIVE_RE.search(text))
            or _var_sensitive(text)
            or any(c.kind == "env" for c in cargo_matches(text))
        )
        if not (sensitive and _cwd_inside_rs(text, projects[pid]["source_dir"])):
            continue
        in_scope.add(target)
        reason = allow.get(target)
        if reason is not None:
            if not reason.strip():
                rows.append(
                    f"{target} is in ALLOW_MISSING_CARGO_CONFIG with an empty reason — an "
                    f"exemption is allowed, a silent one is not"
                )
            continue
        files = (projects[pid].get("task_inputs") or {}).get(name)
        globs = (projects[pid].get("task_input_globs") or {}).get(name)
        if files is None or globs is None:
            rows.append(
                f"{target} reported no `inputFiles`/`inputGlobs` — moon's output shape "
                f"changed, so this assertion cannot be evaluated (treated as a violation, "
                f"never skipped)"
            )
            continue
        if CARGO_CONFIG_INPUT not in set(files) | set(globs):
            rows.append(
                f"{target} runs a compiling cargo command with cwd inside rs/ but does not "
                f"key on {CARGO_CONFIG_INPUT} — a rustflags edit replays its cached result"
            )
    for target in sorted(set(floor) - in_scope):
        rows.append(
            f"FLOOR: A10 examines {len(in_scope)} task(s) and {target} is not among them — "
            f"the derivation or the cwd rule has degraded and would assert nothing"
        )
    for target in sorted(set(floor) & set(allow)):
        rows.append(
            f"FLOOR: {target} is a floor member AND allowlisted — an allowlist that grows to "
            f"cover the floor is how a default-deny gate becomes vacuous"
        )
    for target in sorted(set(allow) - in_scope):
        rows.append(
            f"ALLOW_MISSING_CARGO_CONFIG names {target}, which A10 does not examine — the "
            f"waiver is stale; delete it"
        )
    return rows


def check_dockerfile_locked(root):
    """Return A8 rows for rs/Dockerfile, which moon's task graph cannot see.

    A narrow, line-oriented assertion rather than a general text scan: the file holds one cargo
    line and no prose that mentions a cargo verb, so the false-positive rate that killed the
    general scan does not apply. A missing file is infrastructure, never a silent pass.
    """
    path = root / "rs" / "Dockerfile"
    if not path.is_file():
        raise MoonOutputError(
            f"{path} is absent — A8's Dockerfile assertion cannot be evaluated. If the file "
            f"legitimately moved, update check_dockerfile_locked rather than deleting the check"
        )
    rows = []
    seen = 0
    for lineno, line in enumerate(path.read_text().splitlines(), 1):
        stripped = line.split("#", 1)[0]
        if _dockerfile_env_redirects_cargo(stripped):
            rows.append(
                f"rs/Dockerfile:{lineno} sets CARGO= in an ENV directive, redirecting cargo for "
                f"every later RUN through a tool that cannot take {LOCKED_FLAG}: "
                f"{stripped.strip()}"
            )
            continue
        found = cargo_matches(stripped)
        if not found:
            continue
        # The FLOOR counts LITERAL matches only (SMA-605 review). `seen` exists to catch a
        # Dockerfile that stopped compiling; an `ENV CARGO=…` line redirects cargo but invokes
        # nothing, so letting it increment `seen` would keep the floor quiet after the real
        # `RUN cargo build --locked` was deleted.
        seen += sum(1 for c in found if c.kind == "literal")
        if any(c.kind == "env" for c in found):
            rows.append(
                f"rs/Dockerfile:{lineno} sets CARGO= to redirect cargo through another tool, "
                f"which cannot take {LOCKED_FLAG}: {stripped.strip()}"
            )
        elif LOCKED_FLAG not in stripped:
            rows.append(
                f"rs/Dockerfile:{lineno} reaches cargo without {LOCKED_FLAG}: {stripped.strip()}"
            )
    # The floor, for the reason REQUIRED_LOCKED_TASKS carries: zero matches asserts nothing while
    # still printing PASS.
    if seen == 0:
        rows.append(
            "A8 examines rs/Dockerfile and found no cargo invocation at all — the image build "
            "stopped compiling in this file, so this assertion now covers nothing"
        )
    return rows


def dependabot_expand_member(rs_root, entry):
    """Replay Dependabot's `expand_workspaces` for ONE `[workspace] members` entry.

    A transcription of `cargo/lib/dependabot/cargo/file_fetcher.rb`, not an approximation of it,
    because the whole point of A9 is that Dependabot's expander and Cargo's disagree — a rule
    written in this file's own terms would drift away from the thing it models. Ruby:

        unglobbed_path = (path.split("*").first || "").gsub(%r{(?<=/)[^/]*$}, "")
        repo_contents(dir: unglobbed_path, raise_errors: false)
          .select { |file| file.type == "dir" }
          .map    { |f| f.path.gsub(%r{^/?#{Regexp.escape(dir)}/?}, "") }
          .select { |filename| File.fnmatch?(path, filename) }

    So it lists exactly ONE directory level below the glob's literal prefix. `crates/*/*` yields
    the prefix `crates/`, whose children are `crates/libs`, `crates/services` and
    `crates/bindings` — and `File.fnmatch?("crates/*/*", "crates/libs")` is false for all three,
    because the pattern still needs a `/` the candidate does not have. Zero members.

    Ruby's `File.fnmatch?` without `FNM_PATHNAME` lets `*` cross `/`, and so does Python's
    `fnmatch`, which is why the two agree here. A literal entry (no `*`) is returned as itself:
    Dependabot takes that path verbatim, and so does Cargo.
    """
    if "*" not in entry:
        return [entry]
    prefix = re.sub(r"(?<=/)[^/]*$", "", entry.split("*", 1)[0])
    base = rs_root / prefix if prefix else rs_root
    if not base.is_dir():
        return []
    return sorted(
        candidate
        for child in base.iterdir()
        if child.is_dir() and fnmatch.fnmatchcase(candidate := f"{prefix}{child.name}", entry)
    )


def check_member_globs(root, crates):
    """Return the A9 violation list: workspace crates Dependabot's member expansion cannot reach.

    Cargo's member set is the ONLY thing the rest of this repo cares about, so a `members` entry
    that Dependabot resolves to nothing is invisible everywhere else — `cargo metadata` is
    identical, every Moon task is identical, every other assertion in this file is identical. The
    only tell in production is a Dependabot PR whose `rs/Cargo.lock` shrank, which is how
    `crates/*/*` survived from the workspace's bootstrap to SMA-604.

    An absent manifest or an absent `members` key is infrastructure, never a silent pass: both
    would make the comparison vacuous while still printing PASS.
    """
    path = root / "rs" / "Cargo.toml"
    if not path.is_file():
        raise MoonOutputError(
            f"{path} is absent — A9's member-glob assertion cannot be evaluated. If the workspace "
            f"root legitimately moved, update check_member_globs rather than deleting the check"
        )
    members = (tomllib.loads(path.read_text()).get("workspace") or {}).get("members")
    if not members:
        raise MoonOutputError(
            f"{path} declares no `[workspace] members` — A9 cannot compare an expansion against "
            f"a member set that does not exist"
        )

    rows = []
    reachable = set()
    for entry in members:
        expanded = dependabot_expand_member(root / "rs", entry)
        reachable.update(expanded)
        if not expanded:
            rows.append(
                f"members entry {entry!r} resolves to ZERO members under Dependabot's expander "
                f"(it lists one directory level below {entry.split('*', 1)[0]!r})"
            )

    # `source_dir` is root-relative (`rs/crates/libs/paigasus-kernel`); `members` entries are
    # rs-relative. Compare in the members entries' own frame.
    want = {
        c["source_dir"].split("/", 1)[1]
        for c in crates.values()
        if c["source_dir"].startswith("rs/")
    }
    for missing in sorted(want - reachable):
        rows.append(
            f"{missing} is a workspace crate that Dependabot's member expansion never reaches"
        )

    # The floor, for the reason REQUIRED_LOCKED_TASKS carries: an empty `want` makes the set
    # difference above trivially empty, so A9 would print PASS having compared nothing.
    if not want:
        rows.append(
            "A9 examines rs/Cargo.toml's members against the crates cargo_crates() found and "
            "found NO crates at all — this assertion now covers nothing"
        )
    return rows


def check_ffi_inputs(projects, required=FFI_TASK_INPUTS, floor=REQUIRED_FFI_TASKS):
    """Return the A5 violation list: FFI-compiling tasks that do not key on the workspace files.

    Two halves, and both are load-bearing:

    * DERIVED — any task whose resolved invocation matches an FFI marker must declare `required`.
      This is what covers a future fourth binding task on the day it is added.
    * FLOOR — every task in `floor` must appear in the derived set. Without this a derivation that
      silently stops matching (a renamed flag, an invocation moved behind a wrapper script, a moon
      upgrade dropping `script`) degrades to an empty set and a vacuous PASS.

    The derivation itself lives in `derive_ffi_tasks`, shared with A7.

    Raises MoonOutputError if a task exposes none of a command, a script, or any args.
    """
    matched, a5 = derive_ffi_tasks(projects), []
    for target in sorted(matched):
        pid, _, name = target.partition(":")
        resolved = (projects[pid].get("task_inputs") or {}).get(name)
        if resolved is None:
            a5.append(
                f"{target} reported no `inputFiles` — moon's output shape changed, so this "
                f"assertion cannot be evaluated (treated as a violation, never skipped)"
            )
            continue
        missing = [f for f in required if f not in resolved]
        if missing:
            a5.append(f"{target} inputs omit {', '.join(missing)}")
    for target in sorted(set(floor) - matched):
        a5.append(
            f"{target} is not matched by any FFI marker — the derived set no longer covers it, "
            f"so A5 would assert nothing about it (see FFI_MARKERS)"
        )
    return a5


def rust_closure(projects, pid, seen=None, origin=None):
    """Transitive dependsOn closure of `pid`, restricted to Rust projects.

    Restricted because Moon injects non-Rust build-scope parents into `dependencies`: `contracts`
    arrives via the `contracts:generate` task dep (which is what NON_CARGO_PARENTS already exists
    for) and has neither a `src/` tree nor a Cargo.toml, so an unfiltered closure would demand
    globs matching nothing for four crates.

    Transitive, not direct: affectedness does NOT propagate through `^:build` (SMA-528 F1), so with
    A -> B -> C an edit to C would otherwise reach B and stop.

    `origin` is the crate the walk started from and is excluded at EVERY depth, not just the first.
    Comparing against `pid` instead would only skip a direct self-edge: in a cycle A -> B -> A the
    walk from A would return {B, A}, putting A's own pair in `want` while `observed` deliberately
    excludes it — an `inputs omit <own>/src/**/*` row no declaration could ever satisfy. A self-edge
    deeper in is already covered by `dep in seen`, since `seen` gains `dep` before the recursion.
    """
    seen = set() if seen is None else seen
    origin = pid if origin is None else origin
    for dep in sorted((projects.get(pid) or {}).get("deps") or {}):
        if dep == origin or dep in NON_CARGO_PARENTS or dep not in projects:
            continue
        if projects[dep].get("language") != "rust" or dep in seen:
            continue
        seen.add(dep)
        rust_closure(projects, dep, seen, origin)
    return seen


def check_upstream_inputs(
    projects, allow=None, floor=REQUIRED_CLOSURE_EDGES, tasks=UPSTREAM_INPUT_TASKS
):
    """Return the A6 violation list: crates whose build/test/lint do not key on their upstreams.

    A1-A3 assert dependency EDGES; A4/A5 assert WORKSPACE-level task inputs. A6 asserts per-crate
    UPSTREAM inputs, and it is the only thing standing between a wrong `fileGroups.upstreams` and a
    silent green: a crate's own moon.yml is NOT an input to its tasks (measured, SMA-528 F5 —
    paigasus-kernel-parity-rs:fmt reported hash 12d26cbd before and after a fileGroups edit), so a
    stale or empty group cannot red anything by itself.

    STRICT EQUALITY, not a subset check. A subset check would let a removed dependsOn edge leave
    stale globs in place forever and let a copy-pasted group over-approximate permanently —
    unbounded invisible CI cost, and the positive-superset model SMA-429 deliberately abandoned.
    """
    allow = ALLOW_OVER_APPROXIMATION if allow is None else allow
    a6 = []

    # The crates the per-crate loop below will actually examine. Computed here because the floor
    # has to assert MEMBERSHIP as well as derivation: `rust_closure` filters on each DEPENDENCY's
    # language but never on the ROOT's, so a crate that stopped reporting `language: rust` — a moon
    # toolchain reshuffle, a hand-edited `language:` — drops silently out of the loop while its
    # closure still derives perfectly and the floor still passes.
    examined = {pid for pid, proj in projects.items() if proj.get("language") == "rust"}

    # FLOOR first: if the derivation broke, every per-crate check below is vacuous. Every row here
    # is prefixed `FLOOR:` so the negative control can tell a floor failure from a per-crate one.
    for consumer, required in sorted((floor or {}).items()):
        if consumer not in projects:
            a6.append(f"FLOOR: {consumer} is not in the graph at all")
            continue
        if consumer not in examined:
            a6.append(
                f"FLOOR: {consumer} is not among the {len(examined)} crates A6 examines — it "
                f"stopped reporting `language: rust`, so the per-crate loop skips it entirely"
            )
            continue
        derived = rust_closure(projects, consumer)
        for upstream in sorted(set(required) - derived):
            a6.append(
                f"FLOOR: {consumer}'s derived closure omits {upstream} — the dependsOn "
                f"derivation is broken, so A6 is asserting nothing"
            )

    for pid, proj in sorted(projects.items()):
        if pid not in examined:
            continue
        own = proj["source_dir"]
        want = set()
        for upstream in rust_closure(projects, pid):
            src = projects[upstream]["source_dir"]
            want.add(f"{src}/src/**/*")
            want.add(f"{src}/Cargo.toml")
        for task in tasks:
            declared_files = (proj.get("task_inputs") or {}).get(task, _ABSENT)
            declared_globs = (proj.get("task_input_globs") or {}).get(task, _ABSENT)
            if declared_files is _ABSENT and declared_globs is _ABSENT:
                a6.append(f"{pid} has no `{task}` task (nothing can key on its upstreams)")
                continue
            # One bucket present and the other missing entirely is moon half-reporting the task —
            # folded in with the None case, since both mean "this assertion cannot be evaluated".
            # Never fall through: `_ABSENT` in the union below would be a TypeError at best, and a
            # string sentinel there would silently iterate character-wise.
            if _ABSENT in (declared_files, declared_globs) or None in (
                declared_files,
                declared_globs,
            ):
                a6.append(
                    f"{pid}:{task} reported no inputFiles/inputGlobs — moon's output shape "
                    f"changed, so this assertion cannot be evaluated (treated as a violation, "
                    f"never skipped)"
                )
                continue
            resolved = set(declared_files or []) | set(declared_globs or [])
            # Observed = every entry pointing INTO another crate's tree. The crate's own
            # `src/**/*`, `tests/**/*`, `**/*_test.rs`, and `Cargo.toml` come from
            # .moon/tasks/rust.yml and are not upstreams — excluded below by the `own/` prefix
            # check, not by suffix, so this needs no updating when rust.yml grows a new own-crate
            # glob shape.
            #
            # Deliberately WIDE: any entry under `rs/crates/` that is not the crate's own enters
            # `observed`, regardless of suffix. A prior version matched only `/src/**/*` or
            # `/Cargo.toml`, so a broad `.../paigasus-kernel/**/*` or a
            # `.../paigasus-kernel/tests/**/*` sitting beside the correct pair never entered
            # `observed` and could never surface in the `observed - want` diff below — silently
            # widening what a crate's build/test/lint keys on, the exact direction strict equality
            # was chosen to catch (SMA-429). What this filter still CANNOT see: an over-declared
            # entry living OUTSIDE `rs/crates/**` entirely (e.g. a stray `contracts/**/*`) — such
            # an entry can never be in `want` either, so it is invisible to strict equality in
            # both directions regardless of this filter. Under-declaration is unaffected either
            # way: a missing correct-shaped pair still fails `want - observed`.
            observed = {
                e for e in resolved if e.startswith("rs/crates/") and not e.startswith(f"{own}/")
            }
            for entry in sorted(want - observed):
                a6.append(f"{pid}:{task} inputs omit {entry}")
            for entry in sorted(observed - want):
                # Structural derivation, not suffix-stripping: `entry` may now carry any suffix
                # (`/src/**/*`, `/Cargo.toml`, `/**/*`, `/tests/**/*`, ...), but the first four
                # `/`-separated segments are always `rs/crates/<layer>/<crate>`.
                upstream = "/".join(entry.split("/")[:4])
                if not _allowlisted(allow, pid, upstream):
                    a6.append(f"{pid}:{task} inputs include {entry}, which is not in its closure")
    return a6


def check_wrapper_upstream_inputs(projects, root, floor=REQUIRED_WRAPPER_CLOSURE):
    """Return the A7 violation list: py/ts wrappers that do not key on their upstream crates.

    The cross-stack half of A6. A6 iterates `language == "rust"` only, so the py/ts wrappers —
    whose hand-written `/rs/...` globs ARE the ADR-0005 cross-binding guarantee — were asserted
    by nothing. Note the kernel->wrapper edge specifically IS covered, by one hand-written
    `run.sh` case (`kernel->consumer-tasks`); what was uncovered is every OTHER upstream, any new
    wrapper, and the under-declarations this check's first run found.

    Three deliberate differences from A6:

    * DERIVED TASK SET, not a hand-written one. `derive_ffi_tasks` already finds exactly these
      tasks for A5, so a new wrapper's `napi build` is examined on day one even if it declares no
      inputs at all — which is precisely the bug a hand-written list could not detect.
    * CONTAINMENT, not strict equality. A6's strict equality is right for `fileGroups.upstreams`,
      a mechanical mirror of the closure where anything extra is waste. The wrapper globs are
      hand-written per task and legitimately mixed with non-closure inputs under `rs/crates/` —
      the SMA-433 parity vectors, and each binding's `package.json` / `pyproject.toml`. Strict
      equality would report those correct entries as violations.
    * BOTH BUCKETS, PER TASK. `Cargo.toml` is a path (`inputFiles`), `src/**/*` is a glob
      (`inputGlobs`), and a wrapper's `build` and `test` declare different sets — so a
      one-bucket read, or one that unions across a wrapper's tasks, passes the very mutations
      this check exists to catch.

    `root` is POSITIONAL AND REQUIRED, never defaulted (SMA-560 I3). It gates the `build.rs`
    half — the branch that catches this branch's own headline under-declaration — and a
    `root=None` default made that half opt-in: a call site that simply stopped passing it went
    on printing PASS while the two `paigasus-node-bindings/build.rs` lines could be deleted from
    `ts/packages/paigasus-kernel/moon.yml` for free. Required means the same mistake is now a
    TypeError at the call site instead of a silent downgrade.
    """
    a7 = []
    examined = {}
    for target in sorted(derive_ffi_tasks(projects)):
        pid, _, task = target.partition(":")
        proj = projects.get(pid)
        if proj is None or proj.get("language") == "rust":
            continue
        examined.setdefault(pid, []).append(task)

    # FLOOR first: if the derivation broke, every per-wrapper check below is vacuous. Rows are
    # `FLOOR:`-prefixed so a control can tell a floor failure from a per-wrapper one.
    for pid, required in sorted((floor or {}).items()):
        if pid not in projects:
            a7.append(f"FLOOR: {pid} is not in the graph at all")
            continue
        if pid not in examined:
            a7.append(
                f"FLOOR: {pid} has no task matched by an FFI marker, so A7 examines nothing for "
                f"it — either restore the invocation or update FFI_MARKERS"
            )
            continue
        derived = rust_closure(projects, pid)
        for missing in sorted(required - derived):
            a7.append(f"FLOOR: {pid}'s dependsOn closure no longer derives {missing}")

    for pid, tasks in sorted(examined.items()):
        want = set()
        for upstream in sorted(rust_closure(projects, pid)):
            src = projects[upstream]["source_dir"]
            want.add(f"{src}/src/**/*")
            want.add(f"{src}/Cargo.toml")
            # A build script is compiled by the wrapper's own `napi build`/`maturin` invocation,
            # so a change to it changes what the wrapper links. Only demanded when one exists on
            # disk — which is why `root` is required rather than defaulted (see the docstring).
            if (root / src / "build.rs").is_file():
                want.add(f"{src}/build.rs")
            # SMA-594'. A hand-written .pyi is the interface contract between a PyO3 cdylib and
            # every Python consumer, and it is what basedpyright reads INSTEAD of the Rust. It
            # lives at the crate ROOT, so `{src}/src/**/*` does not match it and nothing that
            # validates it keyed on it. Disk-conditional and globbed, mirroring build.rs above:
            # conditional so the twelve crates without a stub gain no dead demand, globbed so a
            # second stub is covered the day it appears rather than needing a hand-maintained list.
            # Deliberately NOT scoped to py wrappers: a stub at the root of a crate in BOTH
            # closures — paigasus-kernel is the live candidate — would also be demanded of
            # paigasus-kernel-ts, which never reads it. That over-approximation is accepted.
            # A7 asserts CONTAINMENT, so the cost is one extra declared input, not a failure,
            # and a language guard would need its own fixture to prove it fires.
            for stub in sorted((root / src).glob("*.pyi")):
                want.add(f"{src}/{stub.name}")
        for task in sorted(tasks):
            files = (projects[pid].get("task_inputs") or {}).get(task)
            globs = (projects[pid].get("task_input_globs") or {}).get(task)
            if files is None or globs is None:
                a7.append(
                    f"{pid}:{task} reported no `inputFiles`/`inputGlobs` — moon's output shape "
                    f"changed, so this assertion cannot be evaluated (treated as a violation, "
                    f"never skipped)"
                )
                continue
            observed = set(files) | set(globs)
            for entry in sorted(want - observed):
                a7.append(f"{pid}:{task} inputs omit {entry}")
    return a7


def moon_projects():
    """Moon's own resolved graph. Never parse moon.yml — Moon already resolved it.

    `moon query projects` expands `^:build` into concrete targets, so task deps report
    `paigasus-iam-rs:build -> ['paigasus-proto-rs:build', ...]`. Asserting that expansion is
    strictly stronger than grepping for the literal `^:build`: it also catches a hand-written
    per-dependency list that omits an upstream.
    """
    out = subprocess.run(
        ["moon", "query", "projects"], capture_output=True, text=True, check=True
    ).stdout
    projects = {}
    for p in json.loads(out)["projects"]:
        tasks = {}
        task_inputs = {}
        task_input_globs = {}
        invocations = {}
        for name, task in (p.get("tasks") or {}).items():
            tasks[name] = [
                d if isinstance(d, str) else d.get("target")
                for d in (task.get("deps") or [])
            ]
            # `inputFiles` is a path-keyed OBJECT of resolved workspace-relative paths. Preserve the
            # absent-key case as None rather than collapsing it to []: "moon told us nothing" and
            # "moon told us there are none" are different defects, and A4 must fire loudly on the
            # first instead of reporting a confusing missing-file list. Sorted list, not a set, so
            # self_test()'s json round-trip deep-copy keeps working.
            raw = task.get("inputFiles")
            task_inputs[name] = None if raw is None else sorted(raw.keys())
            # SMA-528 — A6 needs BOTH buckets. moon splits resolved inputs by kind: plain paths go
            # to `inputFiles`, globs to `inputGlobs` (measured, SMA-534). `src/**/*` is a glob and
            # `Cargo.toml` is a path, so an A6 that read only one of them could never fire on half
            # of every upstream pair. Same absent-key-is-None contract as `task_inputs`.
            raw_globs = task.get("inputGlobs")
            task_input_globs[name] = None if raw_globs is None else sorted(raw_globs.keys())
            # A5 (SMA-546): the text A5 marker-matches against. Joined from all three fields
            # because moon splits an invocation differently per task form — command-form puts the
            # verb in `args` (command='cargo', args=['clippy', ...]), script-form puts it in
            # `script` (command='touch', script='touch ... && napi build ...'). None when moon
            # reported neither, which check_ffi_inputs escalates to an infra error.
            parts = [task.get("command") or "", task.get("script") or ""]
            parts += [str(a) for a in (task.get("args") or [])]
            joined = " ".join(p for p in parts if p)
            invocations[name] = joined or None
        projects[p["id"]] = {
            "source_dir": p["source"],
            "deps": {d["id"]: d.get("source") for d in (p.get("dependencies") or [])},
            "tasks": tasks,
            "task_inputs": task_inputs,
            "task_input_globs": task_input_globs,
            "invocations": invocations,
            "language": p.get("language"),
        }
    return projects


def cargo_crates(root):
    """Every workspace crate and its in-tree deps, via tomllib.

    tomllib handles all three declaration forms the repo uses — inline table
    (`paigasus-proto = { workspace = true }`), dotted key
    (`paigasus-proto-derive.workspace = true`), and `package =` renames.
    """
    manifests = {}
    for path in sorted((root / "rs" / "crates").rglob("Cargo.toml")):
        data = tomllib.loads(path.read_text())
        package = data.get("package")
        if not package or "name" not in package:
            continue
        manifests[package["name"]] = (path, data)

    crates = {}
    for name, (path, data) in manifests.items():
        deps = set()
        for kind in ("dependencies", "dev-dependencies", "build-dependencies"):
            for key, value in (data.get(kind) or {}).items():
                real = value.get("package", key) if isinstance(value, dict) else key
                if real in manifests and real != name:
                    deps.add(real)
        crates[name] = {
            "source_dir": str(path.parent.relative_to(root)),
            "deps": deps,
        }
    return crates


def self_test():
    """Negative control: each assertion must FIRE on a synthetic violation (SMA-524 D6).

    A gate whose whole value is catching a silent hole must not be able to pass vacuously.
    """
    # Derived, never hand-listed: this fixture is what A4's CLEAN row asserts against, so a
    # hardcoded copy silently breaks every A4 self-test row the day WORKSPACE_LINT_INPUTS grows
    # (it did, on SMA-594). Deriving it means a new workspace input is covered on day one.
    complete_inputs = list(WORKSPACE_LINT_INPUTS)
    ok = {
        "a-rs": {
            "source_dir": "rs/crates/libs/a",
            "deps": {"b-rs": "explicit"},
            "tasks": {
                "build": ["b-rs:build"],
                "test": ["b-rs:build"],
                "lint": ["b-rs:build"],
            },
            "task_inputs": {"build": [], "test": [], "lint": list(complete_inputs)},
        },
        "b-rs": {
            "source_dir": "rs/crates/libs/b",
            "deps": {},
            "tasks": {"build": [], "test": [], "lint": []},
            "task_inputs": {"build": [], "test": [], "lint": list(complete_inputs)},
        },
    }
    # A6 (SMA-528) additions to the SAME fixture, so A1-A5 keep being asserted against it. Sorted
    # lists rather than sets throughout: the negative controls below deep-copy via a json round
    # trip, which a set cannot survive.
    upstream_ok = ["rs/crates/libs/b/src/**/*"]
    # a-rs: manifest in inputFiles, glob in inputGlobs — the real split A6 must span.
    ok["a-rs"]["language"] = "rust"
    ok["a-rs"]["task_inputs"] = {
        "build": ["rs/crates/libs/b/Cargo.toml"],
        "test": ["rs/crates/libs/b/Cargo.toml"],
        "lint": [*complete_inputs, "rs/crates/libs/b/Cargo.toml"],
    }
    ok["a-rs"]["task_input_globs"] = {
        "build": list(upstream_ok), "test": list(upstream_ok), "lint": list(upstream_ok),
    }
    ok["b-rs"]["language"] = "rust"
    ok["b-rs"]["task_input_globs"] = {"build": [], "test": [], "lint": []}
    crates = {
        "a": {"source_dir": "rs/crates/libs/a", "deps": {"b"}},
        "b": {"source_dir": "rs/crates/libs/b", "deps": set()},
    }
    failures = []

    # Floor the floor: REQUIRED_CLOSURE_EDGES, UPSTREAM_INPUT_TASKS and REQUIRED_FFI_TASKS are
    # themselves module constants that can be silently emptied (a bad merge, an over-eager
    # refactor). Every A6 self-test call below passes an explicit `floor=`, so the real
    # REQUIRED_CLOSURE_EDGES is never exercised by anything else here — `REQUIRED_CLOSURE_EDGES =
    # {}` unasserts A6's floor entirely with `--self-test` still exiting 0 (measured). Every A6
    # call also relies on the default `tasks=UPSTREAM_INPUT_TASKS`, and A5's calls rely on the
    # default `floor=REQUIRED_FFI_TASKS`; emptying either is caught TODAY only incidentally, by
    # unrelated row-text assertions elsewhere in this function, which is not a guarantee — so all
    # three get a direct, explicit non-emptiness check here instead.
    #
    # SMA-601 adds a fourth. A8's calls all pass an explicit `floor=` too, so `REQUIRED_LOCKED_TASKS
    # = ()` was MEASURED leaving `--self-test` at rc 0 while A8's real-run floor asserted nothing.
    if not REQUIRED_CLOSURE_EDGES:
        failures.append("REQUIRED_CLOSURE_EDGES is empty — A6's floor would assert nothing")
    if not UPSTREAM_INPUT_TASKS:
        failures.append("UPSTREAM_INPUT_TASKS is empty — A6's per-crate loop would assert nothing")
    if not REQUIRED_FFI_TASKS:
        failures.append("REQUIRED_FFI_TASKS is empty — A5's floor would assert nothing")
    if not REQUIRED_LOCKED_TASKS:
        failures.append("REQUIRED_LOCKED_TASKS is empty — A8's floor would assert nothing")

    if not FMT_TASK_INPUTS:
        failures.append("FMT_TASK_INPUTS is empty — the fmt half of A4 would assert nothing")

    # A4-fmt: the fmt call must span BOTH buckets. `rs/rustfmt.toml` is a literal (inputFiles)
    # and `rs/crates/libs/b/tests/**/*` is a glob (inputGlobs), so a one-bucket check passes
    # while blind to half its own required set — the split A6 already exists because of.
    fmt_ok = json.loads(json.dumps(ok))
    for pid in ("a-rs", "b-rs"):
        fmt_ok[pid]["task_inputs"]["fmt"] = [
            "rs/rustfmt.toml", "rs/rust-toolchain.toml", f"{fmt_ok[pid]['source_dir']}/Cargo.toml",
        ]
        src = fmt_ok[pid]["source_dir"]
        fmt_ok[pid]["task_input_globs"]["fmt"] = [f"{src}/src/**/*", f"{src}/tests/**/*"]
    if check_task_inputs(fmt_ok, crates, "fmt", FMT_TASK_INPUTS) != []:
        failures.append("A4-fmt reported violations on a complete fixture")

    broken = json.loads(json.dumps(fmt_ok))
    broken["a-rs"]["task_input_globs"]["fmt"] = ["rs/crates/libs/a/src/**/*"]
    if not any(
        "tests/**/*" in row
        for row in check_task_inputs(broken, crates, "fmt", FMT_TASK_INPUTS)
    ):
        failures.append("A4-fmt did not fire on a fmt task missing @group(tests)")

    # A4-fmt: the manifest half. `cargo fmt` reads Cargo.toml for its target list and edition, so
    # a fmt task blind to it can serve a cached PASS across a [[bin]] addition (CodeRabbit, PR 174).
    broken = json.loads(json.dumps(fmt_ok))
    broken["a-rs"]["task_inputs"]["fmt"] = ["rs/rustfmt.toml", "rs/rust-toolchain.toml"]
    if not any(
        "Cargo.toml" in row
        for row in check_task_inputs(broken, crates, "fmt", FMT_TASK_INPUTS)
    ):
        failures.append("A4-fmt did not fire on a fmt task missing its crate's Cargo.toml")

    broken = json.loads(json.dumps(fmt_ok))
    broken["a-rs"]["task_inputs"]["fmt"] = ["rs/rust-toolchain.toml"]
    if not any(
        "rustfmt.toml" in row
        for row in check_task_inputs(broken, crates, "fmt", FMT_TASK_INPUTS)
    ):
        failures.append("A4-fmt did not fire on a fmt task missing the rustfmt config")

    # A4's crate-relative fallback is ANCHORED to the crate's own source_dir (SMA-560 M3). The
    # previous unanchored tail match let ANOTHER crate's globs satisfy this one: `a-rs:fmt` counted
    # as covered by `rs/crates/libs/b/src/**/*`, so a crate that lost @group(sources) entirely was
    # still reported green as long as some other crate declared its own.
    broken = json.loads(json.dumps(fmt_ok))
    src_b = fmt_ok["b-rs"]["source_dir"]
    broken["a-rs"]["task_input_globs"]["fmt"] = [f"{src_b}/src/**/*", f"{src_b}/tests/**/*"]
    if not any(
        row == "a-rs:fmt inputs omit src/**/*, tests/**/*"
        for row in check_task_inputs(broken, crates, "fmt", FMT_TASK_INPUTS)
    ):
        failures.append("A4 let one crate's fmt be satisfied by ANOTHER crate's source globs")

    # A5/A7 share one derivation. If they ever diverge, A7 silently stops examining a wrapper
    # while A5 keeps passing — so assert the split function returns exactly what A5 matches.
    ffi_fixture = {
        "w-ts": {
            "source_dir": "ts/packages/w", "deps": {}, "language": "typescript",
            "tasks": {"build": []}, "task_inputs": {"build": []},
            "task_input_globs": {"build": []},
            "invocations": {"build": "touch ../x && pnpm exec napi build --platform"},
        },
        "q-rs": {
            "source_dir": "rs/crates/libs/q", "deps": {}, "language": "rust",
            "tasks": {"build": []}, "task_inputs": {"build": []},
            "task_input_globs": {"build": []},
            "invocations": {"build": "cargo build"},
        },
    }
    if derive_ffi_tasks(ffi_fixture) != {"w-ts:build"}:
        failures.append(
            f"derive_ffi_tasks did not match exactly the FFI-marked task: "
            f"{sorted(derive_ffi_tasks(ffi_fixture))}"
        )

    # Guard the guard (SMA-542). A check that is defined but never invoked on the real run asserts
    # nothing, and no fixture here would notice — self_test calls the check functions directly.
    # This is generic on purpose: it covers a future A8 on the day it is written. It scans BOTH
    # `main` and `collect_findings`, since SMA-560 moved every invocation into the latter.
    real_run_src = inspect.getsource(main) + inspect.getsource(collect_findings)
    unreferenced = sorted(
        name for name in globals()
        if name.startswith("check_") and f"{name}(" not in real_run_src
    )
    if unreferenced:
        failures.append(
            f"the real run never calls {', '.join(unreferenced)} — a check that is defined but "
            f"not invoked asserts nothing (SMA-542)"
        )

    # ...and the name scan above is only HALF the guard (SMA-560 I4). It cannot separate two call
    # sites of the same function, and it never sees `check` at all (no `check_` prefix), so three
    # measured deletions from the findings list left `--self-test` green with a real assertion
    # gone: the `a5` tuple, either `check_task_inputs` tuple, and the `a1`/`a2`/`a3` tuples. Pin
    # the LIST itself — arity first, so a shrunk list says so plainly, then the exact key sequence.
    if not EXPECTED_FINDING_KEYS:
        failures.append("EXPECTED_FINDING_KEYS is empty — the findings floor would assert nothing")
    # SMA-599 — this tmp root holds ONLY rs/Dockerfile. That is safe because `ok` declares
    # no invocation referencing a ci/**/*.sh, so task_script_refs never looks for one. If a
    # future fixture adds such an invocation it MUST also create the script under this tmp
    # root, or task_script_refs raises MoonOutputError and the arity check stops being
    # about arity.
    if any("ci/" in (blob or "") for p in ok.values()
           for blob in (p.get("invocations") or {}).values()):
        failures.append(
            "the arity fixture now references a ci/ script but its tmp root does not create one"
        )
    with tempfile.TemporaryDirectory() as tmp:
        # collect_findings now folds check_dockerfile_locked(root) into a8 and
        # check_member_globs(root, crates) into a9, and BOTH raise on an absent file — write a
        # locked Dockerfile and a members list that reaches `crates`' own source dirs, so this
        # arity check stays about arity. The members list is DERIVED from the same `crates`
        # fixture the call passes, so a fixture edit cannot leave this row asserting an arity
        # failure that is really an a9 violation in disguise.
        tmp_rs = Path(tmp) / "rs"
        tmp_rs.mkdir()
        (tmp_rs / "Dockerfile").write_text("RUN cargo build --release --locked -p paigasus-iam\n")
        member_dirs = sorted(
            c["source_dir"].split("/", 1)[1]
            for c in crates.values()
            if c["source_dir"].startswith("rs/")
        )
        (tmp_rs / "Cargo.toml").write_text(
            "[workspace]\nmembers = [%s]\n" % ", ".join(f'"{d}"' for d in member_dirs)
        )
        collected = collect_findings(ok, crates, Path(tmp))
    if len(collected) != len(EXPECTED_FINDING_KEYS):
        failures.append(
            f"collect_findings returned {len(collected)} entries, expected "
            f"{len(EXPECTED_FINDING_KEYS)} — a check was added or dropped without updating "
            f"EXPECTED_FINDING_KEYS"
        )
    got_keys = tuple(key for key, _, _ in collected)
    if got_keys != EXPECTED_FINDING_KEYS:
        failures.append(
            f"collect_findings reported {got_keys}, expected {EXPECTED_FINDING_KEYS} — a check "
            f"was dropped, added or reordered in the findings list"
        )

    if not REQUIRED_WRAPPER_CLOSURE:
        failures.append("REQUIRED_WRAPPER_CLOSURE is empty — A7's floor would assert nothing")

    # A7 fixture: a ts wrapper depending on a binding crate that depends on the kernel. The
    # wrapper declares the manifest as a literal and the sources as a glob — the same two-bucket
    # split A6 spans — plus one legitimate extra outside its closure (the parity corpus), which
    # containment must ALLOW and strict equality would have wrongly flagged.
    #
    # TWO tasks, and that is load-bearing (SMA-560 I2). A one-task wrapper cannot tell a per-task
    # read from one that unions across the wrapper's tasks, so the per-(project, task) read the
    # docstring names as one of A7's three deliberate differences would be held by nothing. `build`
    # and `test` therefore declare DIFFERENT complete sets, exactly as the real wrappers do, and
    # A7-g below drops an entry from `test` alone that `build` still carries.
    wrap = {
        "k-ts": {
            "source_dir": "ts/packages/k", "deps": {"nb-rs": "explicit"}, "language": "typescript",
            "tasks": {"build": [], "test": []},
            "task_inputs": {
                "build": ["rs/crates/libs/kern/Cargo.toml",
                          "rs/crates/bindings/nb/Cargo.toml"],
                "test": ["rs/crates/libs/kern/Cargo.toml",
                         "rs/crates/bindings/nb/Cargo.toml",
                         "rs/crates/bindings/nb/package.json"],
            },
            "task_input_globs": {
                "build": ["rs/crates/libs/kern/src/**/*",
                          "rs/crates/bindings/nb/src/**/*",
                          "rs/crates/libs/parity/vectors/**/*"],
                "test": ["rs/crates/libs/kern/src/**/*",
                         "rs/crates/bindings/nb/src/**/*"],
            },
            "invocations": {
                "build": "pnpm exec napi build --platform",
                "test": "touch ../x && pnpm exec napi build --platform && vitest run",
            },
        },
        "nb-rs": {
            "source_dir": "rs/crates/bindings/nb", "deps": {"kern-rs": "explicit"},
            "language": "rust", "tasks": {"build": []}, "task_inputs": {"build": []},
            "task_input_globs": {"build": []}, "invocations": {"build": "cargo build"},
        },
        "kern-rs": {
            "source_dir": "rs/crates/libs/kern", "deps": {}, "language": "rust",
            "tasks": {"build": []}, "task_inputs": {"build": []},
            "task_input_globs": {"build": []}, "invocations": {"build": "cargo build"},
        },
    }
    wrap_floor = {"k-ts": {"kern-rs", "nb-rs"}}
    # A7's `root` is required, so every call below passes one. This tempdir holds NO build.rs, so
    # `want` stays at the two-entry-per-upstream shape these rows are written against; the
    # build.rs half gets its own fixture tree in A7-h.
    with tempfile.TemporaryDirectory() as no_build_rs:
        bare = Path(no_build_rs)

        if check_wrapper_upstream_inputs(wrap, bare, floor=wrap_floor) != []:
            failures.append(
                f"A7 reported violations on a complete fixture: "
                f"{check_wrapper_upstream_inputs(wrap, bare, floor=wrap_floor)}"
            )

        # A7-a: a MISSING upstream glob is the dangerous direction and must fire.
        broken = json.loads(json.dumps(wrap))
        broken["k-ts"]["task_input_globs"]["build"] = ["rs/crates/libs/kern/src/**/*"]
        if not any(
            "rs/crates/bindings/nb/src/**/*" in row
            for row in check_wrapper_upstream_inputs(broken, bare, floor=wrap_floor)
        ):
            failures.append("A7 did not fire on a wrapper task missing an upstream's sources")

        # A7-b: the manifest half, which lives in the OTHER bucket. A one-bucket A7 passes this.
        broken = json.loads(json.dumps(wrap))
        broken["k-ts"]["task_inputs"]["build"] = ["rs/crates/bindings/nb/Cargo.toml"]
        if not any(
            "rs/crates/libs/kern/Cargo.toml" in row
            for row in check_wrapper_upstream_inputs(broken, bare, floor=wrap_floor)
        ):
            failures.append("A7 did not fire on a wrapper task missing an upstream's Cargo.toml")

        # A7-c: Rust projects belong to A6, never A7. Double-covering them would make A6's strict
        # equality and A7's containment disagree on the same task.
        #
        # The flipped project is UNDER-DECLARED on purpose (SMA-560 I1). With a CLEAN one this row
        # was vacuous: `!= []` cannot tell "filtered out by the language test" from "examined and
        # found satisfied", so deleting `or proj.get("language") == "rust"` from the examined-set
        # filter kept --self-test green (measured). Emptying both glob buckets means an examined
        # k-ts MUST emit rows, so the empty result now proves the filter ran.
        rusty = json.loads(json.dumps(wrap))
        rusty["k-ts"]["language"] = "rust"
        rusty["k-ts"]["task_input_globs"] = {"build": [], "test": []}
        rusty["k-ts"]["task_inputs"] = {"build": [], "test": []}
        if check_wrapper_upstream_inputs(rusty, bare, floor={}) != []:
            failures.append("A7 examined a Rust project, which is A6's job")

        # A7-d: the FLOOR must fire when the closure derivation degrades to empty. Emptying `deps`
        # also empties `want`, so the per-task loop goes quiet by itself — this MUST match the
        # `FLOOR:` prefix or it passes with the whole floor block deleted (A6-e's lesson).
        broken = json.loads(json.dumps(wrap))
        broken["k-ts"]["deps"] = {}
        if not any(
            row.startswith("FLOOR:") and "k-ts's dependsOn closure no longer derives kern-rs" in row
            for row in check_wrapper_upstream_inputs(broken, bare, floor=wrap_floor)
        ):
            failures.append("A7 floor did not fire on a neutered closure derivation")

        # A7-e: a floor entry naming a project that is not examined at all is a FLOOR violation,
        # never a silent skip — the wrapper's task could have stopped matching an FFI marker.
        # The assertion names THIS branch's own message, never the bare `FLOOR:` prefix: with only
        # the prefix asserted, deleting the branch lets `k-ts` fall through to the
        # closure-derivation branch, which emits a `FLOOR:` row of its own and keeps this control
        # green with the very code it names removed (SMA-542, measured).
        broken = json.loads(json.dumps(wrap))
        broken["k-ts"]["invocations"] = {"build": "echo nothing", "test": "echo nothing"}
        if not any(
            row.startswith("FLOOR:") and "k-ts has no task matched by an FFI marker" in row
            for row in check_wrapper_upstream_inputs(broken, bare, floor=wrap_floor)
        ):
            failures.append("A7 floor did not fire when a wrapper stopped matching any FFI marker")

        # A7-f: a floor entry naming an absent project is a FLOOR violation.
        # Same discrimination as A7-e, in the other direction: an absent project deleted from the
        # `pid not in projects` branch falls straight through to the `pid not in examined` one.
        if not any(
            row.startswith("FLOOR:") and "ghost-ts is not in the graph at all" in row
            for row in check_wrapper_upstream_inputs(wrap, bare, floor={"ghost-ts": {"kern-rs"}})
        ):
            failures.append("A7 floor did not fire on a floor entry naming an absent project")

        # A7-g: the read is PER (project, task) — SMA-560 I2. `test` loses an upstream glob that
        # `build` still declares, so an A7 that unioned a wrapper's tasks would see a complete set
        # and report nothing. The row must NAME `k-ts:test`: asserting only that some row mentions
        # the glob would also pass if the union leaked it out under `k-ts:build`.
        broken = json.loads(json.dumps(wrap))
        broken["k-ts"]["task_input_globs"]["test"] = ["rs/crates/bindings/nb/src/**/*"]
        rows = check_wrapper_upstream_inputs(broken, bare, floor=wrap_floor)
        if not any("k-ts:test inputs omit rs/crates/libs/kern/src/**/*" == row for row in rows):
            failures.append(
                "A7 did not report a per-task under-declaration against the task that carries it "
                "— it may be unioning inputs across the wrapper's tasks"
            )
        if any(row.startswith("k-ts:build inputs omit") for row in rows):
            failures.append("A7 blamed `build` for a shortfall that lives on `test`")

    # A7-h: the build.rs half, which is A7's headline assertion and was previously unreachable
    # from --self-test at all — `root` defaulted to None, so no self-test row could exercise it
    # (SMA-560 I3). A real file on disk under a fixture crate's source_dir is what turns the
    # `is_file()` branch on, so this row needs its own tree.
    with tempfile.TemporaryDirectory() as tmp:
        rooted = Path(tmp)
        (rooted / "rs" / "crates" / "bindings" / "nb").mkdir(parents=True)
        (rooted / "rs" / "crates" / "bindings" / "nb" / "build.rs").write_text("fn main() {}\n")
        rows = check_wrapper_upstream_inputs(wrap, rooted, floor=wrap_floor)
        if not any("inputs omit rs/crates/bindings/nb/build.rs" in row for row in rows):
            failures.append(
                "A7 did not demand an upstream's build.rs that exists on disk — the `root` half "
                "of the check is not asserting anything"
            )
        # ...and it must be demanded of EVERY examined task, not just the first one.
        for task in ("build", "test"):
            if not any(
                f"k-ts:{task} inputs omit rs/crates/bindings/nb/build.rs" == row for row in rows
            ):
                failures.append(f"A7 did not demand nb's build.rs of k-ts:{task}")
        # A crate with NO build.rs on disk must not be demanded one — otherwise every wrapper
        # gains an unsatisfiable row the day this branch is written wrong.
        if any("rs/crates/libs/kern/build.rs" in row for row in rows):
            failures.append("A7 demanded a build.rs for an upstream that has none on disk")

    # A7-i: the .pyi half (SMA-594'). Same disk-conditional shape as A7-h's build.rs, and it needs
    # its own tree for the same reason — the `is_file()`/glob branch is only live when a stub
    # actually exists under an upstream's source_dir.
    with tempfile.TemporaryDirectory() as tmp:
        stubbed = Path(tmp)
        (stubbed / "rs" / "crates" / "bindings" / "nb").mkdir(parents=True)
        (stubbed / "rs" / "crates" / "bindings" / "nb" / "nb.pyi").write_text("def f() -> int: ...\n")
        rows = check_wrapper_upstream_inputs(wrap, stubbed, floor=wrap_floor)
        if not any("inputs omit rs/crates/bindings/nb/nb.pyi" in row for row in rows):
            failures.append(
                "A7 did not demand an upstream's .pyi stub that exists on disk — the stub half "
                "of the check is not asserting anything"
            )
        # Demanded of EVERY examined task, not just the first.
        for task in ("build", "test"):
            if not any(
                f"k-ts:{task} inputs omit rs/crates/bindings/nb/nb.pyi" == row for row in rows
            ):
                failures.append(f"A7 did not demand nb's .pyi of k-ts:{task}")
        # An upstream with NO stub must not be demanded one, or every wrapper gains an
        # unsatisfiable row the day this branch is written wrong.
        if any(row.endswith(".pyi") and "rs/crates/libs/kern/" in row for row in rows):
            failures.append("A7 demanded a .pyi for an upstream that has none on disk")

    # A8 (SMA-601): every task whose resolved invocation reaches cargo must pass --locked.
    # An unlocked one re-resolves and REWRITES an inconsistent lock in place, which is how five
    # Dependabot PRs merged a truncated lock through a green `moon ci`.
    #
    # `k-ts:build` is a WRAPPER match and carries no --locked at all, so the clean baseline proves
    # an allowlist entry is what clears it. A8-f below proves the converse: a wrapper is NOT
    # cleared by a --locked that belongs to some other command in the same script.
    locked_ok = {
        "a-rs": {"invocations": {"lint": "cargo clippy --locked --all-targets"}},
        "b-rs": {"invocations": {"build": "cargo build --locked"}},
        "k-ts": {"invocations": {"build": "pnpm exec napi build --platform"}},
    }
    if check_cargo_locked(locked_ok, allow={"k-ts:build": "napi has no --locked"},
                          floor=("a-rs:lint",)):
        failures.append("A8 reported violations on a clean fixture")

    # A8-a: an unlocked cargo invocation with no allowlist entry must fire, and name the task.
    broken = {"a-rs": {"invocations": {"lint": "cargo clippy --all-targets"}}}
    rows = check_cargo_locked(broken, allow={}, floor=("a-rs:lint",))
    if not any("a-rs:lint" in r and "without --locked" in r for r in rows):
        failures.append("A8 did not fire on an unlocked cargo invocation")

    # A8-b: --frozen is NOT accepted. It implies --offline, which false-reds on a cold cargo
    # cache — the reason the gate itself refuses --offline.
    frozen = {"a-rs": {"invocations": {"lint": "cargo clippy --frozen"}}}
    if not any(
        "without --locked" in r
        for r in check_cargo_locked(frozen, allow={}, floor=("a-rs:lint",))
    ):
        failures.append("A8 accepted --frozen, which implies --offline")

    # A8-c: an allowlist entry with an empty reason must be rejected, like A6-d's.
    rows = check_cargo_locked(broken, allow={"a-rs:lint": ""}, floor=("a-rs:lint",))
    if not any("empty reason" in r for r in rows):
        failures.append("A8 accepted an allowlist entry with an empty reason")

    # A8-d: the FLOOR must fire when the derivation degrades to empty — a derived set that
    # matches nothing asserts nothing while still printing PASS (the A5 lesson).
    rows = check_cargo_locked({"a-rs": {"invocations": {"lint": "echo nothing"}}},
                              allow={}, floor=("a-rs:lint",))
    if not any("A8 examines" in r for r in rows):
        failures.append("A8 floor did not fire when a required task stopped matching")

    # A8-e: an absent invocation is infra-shaped, never a silent skip. Mirrors A5.
    try:
        check_cargo_locked({"a-rs": {"invocations": {"lint": None}}}, allow={},
                           floor=("a-rs:lint",))
        failures.append("A8 did not raise infra on a task with no command and no script")
    except MoonOutputError:
        pass

    # A8-f: THE ANTI-VACUITY ROW for the wrapper half, and the reason A8 does not test the blob
    # uniformly. `paigasus-kernel-ts:build` really does run an unlocked `napi build` beside a
    # `wasm-pack build ... -- --locked`; a blob-level `--locked in blob` test greens it while
    # napi still re-resolves and repairs the lock. A wrapper match must demand an allowlist entry
    # REGARDLESS of a --locked elsewhere in the script.
    stray = {
        "k-ts": {
            "invocations": {
                "build": (
                    "pnpm exec napi build --platform && wasm-pack build . --release "
                    "-- --locked"
                )
            }
        }
    }
    rows = check_cargo_locked(stray, allow={}, floor=("k-ts:build",))
    if not any("k-ts:build" in r and "through a wrapper" in r for r in rows):
        failures.append(
            "A8 accepted a wrapper task on a stray --locked belonging to another command in the "
            "same script — the wrapper's own cargo call is still unlocked"
        )

    # ...and the wrapper rule must still be SATISFIABLE by an allowlist entry, or A8-f would be
    # proving an unfixable row rather than a correct one.
    if check_cargo_locked(stray, allow={"k-ts:build": "napi has no --locked"},
                          floor=("k-ts:build",)):
        failures.append("A8 did not clear a wrapper task that carries an allowlist entry")

    # A8-g: rs/Dockerfile is outside moon's view, so it takes a narrow text assertion of its own.
    # One RUN line, one verb — none of the prose-collision risk a general text scan carries.
    with tempfile.TemporaryDirectory() as tmp:
        rs = Path(tmp) / "rs"
        rs.mkdir()
        (rs / "Dockerfile").write_text("RUN cargo build --release --locked -p paigasus-iam\n")
        if check_dockerfile_locked(Path(tmp)):
            failures.append("A8 reported a violation on a locked Dockerfile")
        (rs / "Dockerfile").write_text("RUN cargo build --release -p paigasus-iam\n")
        if not check_dockerfile_locked(Path(tmp)):
            failures.append("A8 did not fire on an unlocked Dockerfile cargo build")
        # The FLOOR, for the reason REQUIRED_LOCKED_TASKS carries: a Dockerfile that stopped
        # invoking cargo at all leaves this assertion covering nothing while still printing PASS.
        # Untested until CodeRabbit's SMA-601 local review; the floor existed but nothing proved
        # it fires, which is the same defect class the floor itself guards against.
        (rs / "Dockerfile").write_text("FROM scratch\nCOPY --from=build /out /out\n")
        rows = check_dockerfile_locked(Path(tmp))
        if not any("A8 examines rs/Dockerfile" in r for r in rows):
            failures.append("A8's Dockerfile floor did not fire on a file with no cargo call")
        # SMA-605 — the Dockerfile takes the merged list too, but its FLOOR counts LITERAL
        # matches only. Counting merged matches would let an ENV line satisfy `seen > 0` after
        # the real `RUN cargo build --locked` was deleted, which is floor-satisfied-by-a-
        # non-invocation — the vacuity mode this file guards against everywhere else.
        (rs / "Dockerfile").write_text("ENV CARGO=/usr/local/bin/cargo CARGO_HOME=/cargo\n")
        rows = check_dockerfile_locked(Path(tmp))
        if not any("A8 examines rs/Dockerfile" in r for r in rows):
            failures.append(
                "A8's Dockerfile floor was satisfied by a CARGO= line — a redirection is not an "
                "invocation, and the floor now covers nothing"
            )
        if not any("CARGO=" in r and "redirect" in r for r in rows):
            failures.append("A8 did not report a CARGO= redirection in rs/Dockerfile")
        # ...but `CARGO` must be an assignment KEY, not text inside a value. A bare `\bCARGO=`
        # over the raw line reported this benign directive and would have red CI (CodeRabbit).
        (rs / "Dockerfile").write_text(
            'ENV LABEL="CARGO=/usr/bin/cargo"\nRUN cargo build --locked\n'
        )
        if check_dockerfile_locked(Path(tmp)):
            failures.append(
                "A8 reported `ENV LABEL=\"CARGO=...\"` — CARGO inside a quoted VALUE is not an "
                "assignment key and redirects nothing"
            )
        (rs / "Dockerfile").write_text('RUN "$CARGO_BIN" build --release\n')
        if not any("without --locked" in r for r in check_dockerfile_locked(Path(tmp))):
            failures.append("A8 did not fire on an indirect unlocked Dockerfile cargo build")
        (rs / "Dockerfile").unlink()
        try:
            check_dockerfile_locked(Path(tmp))
            failures.append("A8 did not raise infra on a missing rs/Dockerfile")
        except MoonOutputError:
            pass

    # SMA-599 — A8's script arm.
    with tempfile.TemporaryDirectory() as tmp:
        probe = Path(tmp) / "ci" / "probe"
        probe.mkdir(parents=True)
        (probe / "run.sh").write_text("cd rs\ncargo update -w\ncargo build --locked\n")
        fixture = {
            "repo": {
                "source_dir": ".", "deps": {}, "tasks": {},
                "task_inputs": {}, "task_input_globs": {},
                "invocations": {"g": "bash ci/probe/run.sh"},
            },
        }
        # Fires on the unlocked resolving line, and NOT on the locked one.
        rows = check_cargo_locked_scripts(fixture, Path(tmp), allow={})
        if len(rows) != 1 or "cargo update -w" not in rows[0]:
            failures.append(f"A8's script arm did not report exactly the unlocked line: {rows}")

        # A waiver keyed by unique TEXT clears it.
        allow_ok = {("ci/probe/run.sh", "cargo update -w"): "deliberate lock writer"}
        if check_cargo_locked_scripts(fixture, Path(tmp), allow=allow_ok):
            failures.append("A8's script arm ignored a valid text-keyed waiver")

        # An empty reason is itself a row.
        allow_bare = {("ci/probe/run.sh", "cargo update -w"): "  "}
        if not any("empty reason" in r for r in
                   check_cargo_locked_scripts(fixture, Path(tmp), allow=allow_bare)):
            failures.append("A8's script arm accepted a waiver with an empty reason")

        # A STALE waiver — text no longer present — must be reported, not ignored.
        allow_stale = dict(allow_ok)
        allow_stale[("ci/probe/run.sh", "cargo vendor")] = "gone"
        if not any("matches no line" in r for r in
                   check_cargo_locked_scripts(fixture, Path(tmp), allow=allow_stale)):
            failures.append("A8's script arm did not report a stale waiver entry")

        # A waiver whose text occurs TWICE is ambiguous and must be rejected.
        (probe / "run.sh").write_text("cargo update -w\ncargo update -w\n")
        if not any("occurs 2 times" in r for r in
                   check_cargo_locked_scripts(fixture, Path(tmp), allow=allow_ok)):
            failures.append("A8's script arm accepted a waiver text that is not unique")

        # …but the ambiguity count reads REPORTING rows only (SMA-599 review). Since
        # `_classify_shell_line` emits one row per INVOCATION, a segment holding a locked and
        # an unlocked call yields two rows carrying the same segment text. Exactly one of them
        # reports, so the waiver is unambiguous; counting the locked row would red the honest
        # waiver `ci/actionlint/run.sh:3715` needs. This is the live corpus shape.
        (probe / "run.sh").write_text(
            'x="${s/cargo metadata --locked --format-version 1/cargo metadata '
            '--format-version 1}"\n'
        )
        pair_text = (
            'x="${s/cargo metadata --locked --format-version 1/cargo metadata '
            '--format-version 1}"'
        )
        rows = check_cargo_locked_scripts(fixture, Path(tmp), allow={})
        if len(rows) != 1:
            failures.append(
                f"A8's script arm did not report exactly the unlocked half of a two-invocation "
                f"segment: {rows}"
            )
        rows = check_cargo_locked_scripts(
            fixture, Path(tmp), allow={("ci/probe/run.sh", pair_text): "prose, not a call"}
        )
        if rows:
            failures.append(
                f"A8's script arm called a waiver ambiguous because the segment ALSO holds a "
                f"locked invocation, which needs no waiver: {rows}"
            )


        # SMA-605 — the indirect arms, through the real script scanner.
        indirect = probe / "indirect.sh"
        indirect.write_text(
            '#!/usr/bin/env bash\n'
            '"$CARGO_BIN" build\n'                       # 2: reports
            '"$CARGO_BIN" build --locked\n'              # 3: clean
            '"$CARGO_BIN" metadata --no-deps\n'          # 4: clean, the D4 carve-out
            'CARGO=/p release-plz update\n'              # 5: reports, wrapper rule
            'CARGO=/p release-plz update --locked\n'     # 6: reports ANYWAY
            'out="$(cd x && CARGO=/p tool update)"\n'    # 7: reports, inside $( )
            'export CARGO=/p\n'                          # 8: clean, nothing to run
        )
        got = {
            (line.lineno, line.kind)
            for line in script_cargo_lines(indirect)
            if _row_reports(line)
        }
        want = {(2, "var"), (5, "env"), (6, "env"), (7, "env")}
        if got != want:
            failures.append(
                f"A8's script arm reports {sorted(got)} on the indirect fixture, expected "
                f"{sorted(want)}"
            )
        # The `--no-deps` carve-out must key on the VERB, not on CARGO_METADATA_RE: the latter
        # needs a literal lowercase `cargo` and never fires for `"$CARGO_BIN" metadata`.
        if any(line.lineno == 4 and line.resolves for line in script_cargo_lines(indirect)):
            failures.append(
                "A8 treats `\"$CARGO_BIN\" metadata --no-deps` as resolving — the carve-out is "
                "still keyed on CARGO_METADATA_RE rather than on the matched verb (SMA-599 D4)"
            )

        # THE WAIVER ROUND TRIP, and the only fixture that pins the waiver-health loop's half of
        # the kind rule. MEASURED: with `_row_reports` used at emission but the hits predicate
        # left kind-blind, every other fixture here still passes — the row is emitted, a reviewer
        # adds a waiver, emission clears, and the health loop then finds no hits and calls the
        # honest waiver STALE. The line is then permanently red with no escape but rewriting it.
        indirect_fixture = {
            "repo": {
                "source_dir": ".", "deps": {}, "tasks": {},
                "task_inputs": {}, "task_input_globs": {},
                "invocations": {"i": "bash ci/probe/indirect.sh"},
            },
        }
        # Unwaived first: the EMISSION loop must report every env row, including the one whose
        # TOOL carries --locked. Without this the emission half can be made kind-blind on its own
        # and every other fixture here still passes (MEASURED) — the `got`/`want` set above reads
        # `_row_reports` directly and never runs check_cargo_locked_scripts.
        # The emitted row must carry the EXACT waiver key. The waiver dict is keyed on the full
        # stripped segment, so a truncated message means a long segment's key can never be copied
        # out of the gate's own output and the line is unwaivable (CodeRabbit PR review).
        long_seg = 'CARGO=/p tool update ' + '--flag-that-makes-this-segment-long ' * 4
        long_sh = probe / "long.sh"
        long_sh.write_text(long_seg + "\n")
        long_rows = [
            line for line in script_cargo_lines(long_sh) if _row_reports(line)
        ]
        if len(long_rows) != 1:
            failures.append(f"the long-segment fixture did not produce one row: {long_rows}")
        else:
            key = long_rows[0].segment.strip()
            long_fixture = {
                "repo": {
                    "source_dir": ".", "deps": {}, "tasks": {},
                    "task_inputs": {}, "task_input_globs": {},
                    "invocations": {"L": "bash ci/probe/long.sh"},
                },
            }
            emitted = check_cargo_locked_scripts(long_fixture, Path(tmp), allow={})
            if not any(key in r for r in emitted):
                failures.append(
                    f"A8's row truncates a {len(key)}-char segment, so its waiver key cannot be "
                    f"copied from the gate output and the line is unwaivable: {emitted}"
                )

        rows = check_cargo_locked_scripts(indirect_fixture, Path(tmp), allow={})
        if not any("indirect.sh:6" in r and "sets CARGO=" in r for r in rows):
            failures.append(
                f"A8's script arm did not report `CARGO=/p release-plz update --locked` — the "
                f"tool's own --locked cleared an env row, which no flag can do: {rows}"
            )

        waived = {
            ("ci/probe/indirect.sh", "CARGO=/p release-plz update"): "reviewed redirection",
            ("ci/probe/indirect.sh", "CARGO=/p release-plz update --locked"): "reviewed too",
            ("ci/probe/indirect.sh", '"$CARGO_BIN" build'): "reviewed indirect build",
            # Note the trailing `)"`: COMMAND_SPLIT_RE splits INSIDE the substitution, so the
            # segment carries the closing bracket and quote. Copied from the gate's own output —
            # a hand-typed approximation matches nothing and reads as a stale waiver.
            ("ci/probe/indirect.sh", 'CARGO=/p tool update)"'): "reviewed substitution body",
        }
        rows = check_cargo_locked_scripts(indirect_fixture, Path(tmp), allow=waived)
        if rows:
            failures.append(
                f"A8's script arm did not fully clear the indirect fixture under its waivers — "
                f"the waiver-health loop and the emission loop disagree about which rows report: "
                f"{rows}"
            )
    # SMA-605 — the source resolver. EXECUTION ONLY: a bare `ci/**/*.sh` mention in script text
    # is NOT followed, measured at six edges across the real corpus, every one a comment or a
    # pin-array string constant, one new waiver and ZERO true positives (spec M10).
    with tempfile.TemporaryDirectory() as tmp:
        sroot = Path(tmp)
        (sroot / "ci" / "eco").mkdir(parents=True)
        (sroot / "ci" / "run.sh").write_text(
            'HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"\n'
            'ECO="a"\n'
            'ECO="$2"\n'
            "# see ci/other/run.sh for the idiom\n"
            # ...and an EXECUTABLE bare mention, which is the real corpus shape: every one of the
            # six measured prose edges is a comment or a pin-array string CONSTANT, and a
            # constant is executable text. A comment-only fixture stopped asserting the rule the
            # moment _executable_text started stripping comments (MEASURED — M25 survived).
            "REQUIRED=('bash ci/other/run.sh --self-test')\n"
            'source "$HERE/eco/$ECO.sh"\n'
        )
        (sroot / "ci" / "eco" / "a.sh").write_text("cargo build --locked\n")
        (sroot / "ci" / "eco" / "b.sh").write_text("cargo build --locked\n")
        got = sorted(x.name for x in script_source_refs(sroot / "ci" / "run.sh", sroot))
        if got != ["a.sh", "b.sh"]:
            failures.append(
                f"script_source_refs resolved {got}, expected ['a.sh', 'b.sh'] — a variable "
                f"reassigned more than once must GLOB, not resolve to its first value"
            )
        # The bare `ci/other/run.sh` mention in the comment must not appear. It does not exist,
        # so following it would RAISE rather than pass quietly.
        if any("other" in str(x) for x in script_source_refs(sroot / "ci" / "run.sh", sroot)):
            failures.append("script_source_refs followed a bare mention in a comment")

        # A cycle must terminate. Relative targets, deliberately: SOURCE_STMT_RE captures
        # `([^"\'\\s;&|]+)`, so a `source "$(dirname "${BASH_SOURCE[0]}")/b.sh"` target is cut at
        # the first space and never resolves. The one real statement in the tree has no space.
        (sroot / "ci" / "eco" / "a.sh").write_text("source ./b.sh\n")
        (sroot / "ci" / "eco" / "b.sh").write_text("source ./a.sh\ncargo build\n")
        proj = {
            "repo": {
                "source_dir": ".", "deps": {}, "tasks": {},
                "task_inputs": {"t": []}, "task_input_globs": {"t": []},
                "invocations": {"t": "bash ci/run.sh"},
            },
        }
        try:
            closure = task_script_closure(proj, sroot, "repo:t")
        except RecursionError:
            failures.append("task_script_closure recursed on a source cycle")
            closure = []
        if len(closure) != len({x.resolve() for x in closure}):
            failures.append("task_script_closure returned a duplicate on a source cycle")
        if not any(x.name == "b.sh" for x in closure):
            failures.append(
                "task_script_closure did not reach a script two levels down — the closure is "
                "one level deep, so a cargo call in a sourced module stays invisible"
            )

        # A `source` inside a HEREDOC BODY is not executed, so it must not resolve — and it must
        # not abort the gate. MEASURED before the fix: `SOURCE_STMT_RE` over RAW text matched the
        # body line and raised MoonOutputError on the absent target, an infrastructure failure on
        # a benign script (CodeRabbit PR review).
        (sroot / "ci" / "eco" / "real.sh").write_text("cargo build --locked\n")
        (sroot / "ci" / "run.sh").write_text(
            "cat <<'EOF'\n"
            "source ./missing.sh\n"
            "EOF\n"
            "# source ./also-missing.sh\n"
            "source ./eco/real.sh\n"
        )
        try:
            heredoc_got = sorted(x.name for x in script_source_refs(sroot / "ci" / "run.sh", sroot))
        except MoonOutputError:
            heredoc_got = ["<raised>"]
        if heredoc_got != ["real.sh"]:
            failures.append(
                f"script_source_refs resolved {heredoc_got} on a script whose only other "
                f"`source` sits in a heredoc body — it is scanning RAW text, not executable text"
            )
        (sroot / "ci" / "run.sh").write_text(
            'HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"\n'
            'ECO="a"\n'
            'ECO="$2"\n'
            "# see ci/other/run.sh for the idiom\n"
            # ...and an EXECUTABLE bare mention, which is the real corpus shape: every one of the
            # six measured prose edges is a comment or a pin-array string CONSTANT, and a
            # constant is executable text. A comment-only fixture stopped asserting the rule the
            # moment _executable_text started stripping comments (MEASURED — M25 survived).
            "REQUIRED=('bash ci/other/run.sh --self-test')\n"
            'source "$HERE/eco/$ECO.sh"\n'
        )
        (sroot / "ci" / "eco" / "real.sh").unlink()

        # A RELATIVE script path must resolve the same way an absolute one does. `$HERE` expands
        # to `str(path.parent)`, so without the `resolve()` at entry the relative expansion made
        # the `not candidate.is_absolute()` branch prepend the parent a SECOND time and every
        # source resolved to nothing. It failed LOUD rather than quietly, but a guard with no
        # fixture is a guard nobody notices deleting.
        cwd = os.getcwd()
        try:
            os.chdir(sroot)
            rel_got = sorted(x.name for x in script_source_refs(Path("ci/run.sh"), sroot))
        except MoonOutputError:
            rel_got = ["<raised>"]
        finally:
            os.chdir(cwd)
        if rel_got != ["a.sh", "b.sh"]:
            failures.append(
                f"script_source_refs on a RELATIVE path resolved {rel_got}, expected "
                f"['a.sh', 'b.sh'] — the entry `resolve()` is gone and `$HERE` expands relative"
            )

        # A source pointing OUTSIDE the repo resolves to nothing, by the containment filter.
        # `/etc/profile` is not a gate script, and scanning one would put text nobody reviews
        # into A8's corpus — and on a developer machine it would differ from CI.
        with tempfile.TemporaryDirectory() as outside:
            stray = Path(outside) / "stray.sh"
            stray.write_text("cargo build\n")
            (sroot / "ci" / "run.sh").write_text(f"source {stray}\n")
            try:
                script_source_refs(sroot / "ci" / "run.sh", sroot)
                failures.append(
                    "script_source_refs followed a source OUTSIDE the repo — the containment "
                    "filter is gone, and A8's corpus now includes unreviewed text"
                )
            except MoonOutputError:
                pass

        # A source that resolves to nothing is infrastructure, never a silent skip.
        (sroot / "ci" / "run.sh").write_text('source "$HERE/nope/absent.sh"\n')
        try:
            script_source_refs(sroot / "ci" / "run.sh", sroot)
            failures.append("script_source_refs did not raise on a source resolving to nothing")
        except MoonOutputError:
            pass

    # A SYMLINKED root must not crash the consumer. `task_script_refs` builds `root / rel` and
    # keeps the caller's form; `script_source_refs` resolves. Before task_script_closure resolved
    # every member, the closure held one path in link form and one in real form, and
    # `path.relative_to(root)` raised ValueError — which is NOT in INFRA_ERRORS, so it escaped as
    # a traceback rather than the rc-2 classification (MEASURED, CodeRabbit PR review). macOS
    # makes this reachable in ordinary use: /tmp is a symlink to /private/tmp.
    with tempfile.TemporaryDirectory() as tmp:
        link_root, real_root = Path(tmp) / "link", Path(tmp) / "real"
        (real_root / "ci" / "eco").mkdir(parents=True)
        os.symlink(real_root, link_root)
        (real_root / "ci" / "run.sh").write_text("source ./eco/a.sh\n")
        (real_root / "ci" / "eco" / "a.sh").write_text("cargo build\n")
        linked = {
            "repo": {
                "source_dir": ".", "deps": {}, "tasks": {},
                "task_inputs": {"t": []}, "task_input_globs": {"t": []},
                "invocations": {"t": "bash ci/run.sh"},
            },
        }
        try:
            link_rows = check_cargo_locked_scripts(linked, link_root, allow={})
        except ValueError as exc:
            link_rows = []
            failures.append(
                f"A8's script arm raised ValueError on a SYMLINKED root — the closure mixes path "
                f"forms and ValueError is not in INFRA_ERRORS, so it escapes as a traceback: {exc}"
            )
        if not any("ci/eco/a.sh" in r for r in link_rows):
            failures.append(
                f"A8's script arm lost the sourced module under a symlinked root: {link_rows}"
            )

    # The resolver's FLOOR: a rename must red, not silently empty the closure.
    _root = Path(__file__).resolve().parents[2]
    if not check_sourced_scripts(_root, required={"ci/release-parity/run.sh": ("ci/nope.sh",)}):
        failures.append("check_sourced_scripts did not fire on a wrong expected set")
    if check_sourced_scripts(_root):
        failures.append(
            "check_sourced_scripts reports on the REAL corpus — REQUIRED_SOURCED_SCRIPTS no "
            "longer matches what ci/release-parity/run.sh sources"
        )

    if not ALLOW_UNLOCKED_CARGO_SCRIPT:
        failures.append("ALLOW_UNLOCKED_CARGO_SCRIPT is empty — its stale-entry rule asserts nothing")

    # The waivers' PREMISE, asserted. Both entries rest on "the Moon task never passes
    # --write"; adding it would make them silently wrong.
    write_fixture = {"repo": {"invocations": {"version-lockstep": "bash x.sh --write"}}}
    if not check_version_lockstep_no_write(write_fixture):
        failures.append("check_version_lockstep_no_write missed a --write in the task blob")
    clean_fixture = {"repo": {"invocations": {"version-lockstep": "bash x.sh --self-test"}}}
    if check_version_lockstep_no_write(clean_fixture):
        failures.append("check_version_lockstep_no_write fired on a task that passes no --write")
    if check_version_lockstep_no_write({"repo": {"invocations": {}}}) == []:
        failures.append("check_version_lockstep_no_write treated an absent task as a pass")
    # A9: Dependabot's member expander, replayed against a synthetic tree. The whole assertion
    # rests on `dependabot_expand_member` modelling the Ruby faithfully, so the fixture exercises
    # the expander directly as well as the check that consumes it.
    with tempfile.TemporaryDirectory() as tmp:
        root9 = Path(tmp)
        rs9 = root9 / "rs"
        for d in ("crates/libs/kernel", "crates/services/gateway", "crates/bindings/wasm"):
            (rs9 / d).mkdir(parents=True)
        crates9 = {
            "kernel": {"source_dir": "rs/crates/libs/kernel", "deps": set()},
            "gateway": {"source_dir": "rs/crates/services/gateway", "deps": set()},
            "wasm": {"source_dir": "rs/crates/bindings/wasm", "deps": set()},
        }

        def write_members(entries):
            (rs9 / "Cargo.toml").write_text(
                "[workspace]\nmembers = [%s]\n" % ", ".join(f'"{e}"' for e in entries)
            )

        # The expander itself, on the two forms that matter. This is the measurement the whole
        # check is built on: if these two ever agree, A9 is asserting nothing real.
        if dependabot_expand_member(rs9, "crates/*/*") != []:
            failures.append(
                "A9's expander resolved a two-level glob to something — it no longer models "
                "Dependabot's one-directory-level listing, so A9 proves nothing"
            )
        if dependabot_expand_member(rs9, "crates/libs/*") != ["crates/libs/kernel"]:
            failures.append("A9's expander failed to resolve a one-level glob")

        # Clean: three one-level globs reach all three crate directories.
        write_members(["crates/bindings/*", "crates/libs/*", "crates/services/*"])
        if check_member_globs(root9, crates9):
            failures.append("A9 reported a violation on member globs that reach every crate")

        # The SMA-604 regression itself: one two-level glob, zero members, every crate missed.
        write_members(["crates/*/*"])
        rows = check_member_globs(root9, crates9)
        if not any("resolves to ZERO members" in r for r in rows):
            failures.append("A9 did not fire on a two-level members glob")
        if not any("never reaches" in r and "crates/libs/kernel" in r for r in rows):
            failures.append("A9 did not report the crates a two-level glob leaves unreachable")

        # A glob that resolves NON-empty but still misses a directory. Without this row, a check
        # that only tested the zero-resolve case would pass a `members` list that silently drops
        # one crate directory — the same shrunken-sandbox failure, one crate at a time.
        write_members(["crates/libs/*", "crates/services/*"])
        rows = check_member_globs(root9, crates9)
        if any("resolves to ZERO members" in r for r in rows):
            failures.append("A9 reported a zero-resolve row for globs that both resolve")
        if not any("never reaches" in r and "crates/bindings/wasm" in r for r in rows):
            failures.append("A9 did not fire on a members list that omits a crate directory")

        # A literal (glob-free) entry is taken verbatim by both expanders.
        write_members(["crates/libs/kernel", "crates/services/gateway", "crates/bindings/wasm"])
        if check_member_globs(root9, crates9):
            failures.append("A9 reported a violation on literal, glob-free members entries")

        # The FLOOR, for the reason A8's Dockerfile floor carries: with no crates the set
        # difference is trivially empty and A9 would print PASS having compared nothing.
        write_members(["crates/libs/*"])
        rows = check_member_globs(root9, {})
        if not any("A9 examines" in r for r in rows):
            failures.append("A9's floor did not fire when cargo_crates() found no crates")

        # Both infra shapes: an absent manifest and a manifest with no `members` key. Either one
        # would otherwise make the comparison vacuous while still printing PASS.
        (rs9 / "Cargo.toml").write_text("[workspace]\nresolver = \"3\"\n")
        try:
            check_member_globs(root9, crates9)
            failures.append("A9 did not raise infra on a workspace with no `members` key")
        except MoonOutputError:
            pass
        (rs9 / "Cargo.toml").unlink()
        try:
            check_member_globs(root9, crates9)
            failures.append("A9 did not raise infra on a missing rs/Cargo.toml")
        except MoonOutputError:
            pass

    a1, a2, a3 = check(ok, crates)
    if (a1, a2, a3) != ([], [], []):
        failures.append(f"clean fixture reported violations: {a1} {a2} {a3}")

    # A1: drop the project edge, keep the Cargo dep.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["deps"] = {}
    if not check(broken, crates)[0]:
        failures.append("A1 did not fire on a missing project edge")

    # A2: a hand-declared edge with no Cargo backing.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["deps"]["ghost-rs"] = "explicit"
    if not check(broken, crates)[1]:
        failures.append("A2 did not fire on an unbacked explicit edge")

    # A3: the edge exists but the upstream build is not scheduled — the exact hole the
    # project-level affected-graph guard is structurally blind to (SMA-429 F3).
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["tasks"] = {"build": [], "test": [], "lint": []}
    if not check(broken, crates)[2]:
        failures.append("A3 did not fire on an unscheduled upstream build")

    # An ABSENT task key is a different defect from a task that exists but omits the dep, and the
    # violation text has to say which — otherwise the first crate to drop or rename `lint` is told
    # to "add '^:build'" to a task it does not have (SMA-526).
    broken = json.loads(json.dumps(ok))
    del broken["a-rs"]["tasks"]["lint"]
    if not any("has no `lint` task" in row for row in check(broken, crates)[2]):
        failures.append("A3 did not distinguish an absent task from a missing dep")

    # An implicit edge is just as valid as an explicit one — A2 must not flag it.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["deps"] = {"b-rs": "implicit"}
    if check(broken, crates)[1]:
        failures.append("A2 wrongly flagged an implicit (toolchain-inferred) edge")

    # The allowlist exempts on the strength of its REASON, not on bare membership: a blank reason is
    # an unreviewable exemption and must not silence A2.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["deps"]["ghost-rs"] = "explicit"
    if not check(broken, crates, allow={("a-rs", "ghost-rs"): "   "})[1]:
        failures.append("A2 did not fire on an allowlist entry with a blank reason")
    if check(broken, crates, allow={("a-rs", "ghost-rs"): "a documented reason"})[1]:
        failures.append("A2 fired despite a properly-reasoned allowlist entry")

    # A4 (SMA-534): the workspace-level lint inputs must be DECLARED for every crate. Distinct from
    # A1-A3, which are about dependency edges — a crate can have a perfect edge set and still be
    # blind to a Cargo.lock bump.
    if check_task_inputs(ok, crates, "lint", WORKSPACE_LINT_INPUTS):
        failures.append("A4 reported violations on the clean fixture")

    # Fires when a required file is missing from the declared inputs.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["task_inputs"]["lint"] = [
        f for f in WORKSPACE_LINT_INPUTS if f != "rs/rust-toolchain.toml"
    ]
    rows = check_task_inputs(broken, crates, "lint", WORKSPACE_LINT_INPUTS)
    if not rows:
        failures.append("A4 did not fire on a missing workspace lint input")
    elif not any("rs/rust-toolchain.toml" in row for row in rows):
        failures.append("A4 fired but did not name the missing file")

    # Fires for a crate with NO in-tree deps. A3 is guarded by `if want:` and never reaches such a
    # crate; A4 must not copy that shape, or four of the thirteen real crates go unasserted while
    # the negative control stays green.
    broken = json.loads(json.dumps(ok))
    broken["b-rs"]["task_inputs"]["lint"] = []
    if not any(
        row.startswith("b-rs")
        for row in check_task_inputs(broken, crates, "lint", WORKSPACE_LINT_INPUTS)
    ):
        failures.append("A4 did not fire for a dep-free crate (it inherited A3's `if want:` guard)")

    # An ABSENT lint task is a different defect from a lint task with incomplete inputs.
    broken = json.loads(json.dumps(ok))
    del broken["a-rs"]["task_inputs"]["lint"]
    if not any(
        "has no `lint` task" in row
        for row in check_task_inputs(broken, crates, "lint", WORKSPACE_LINT_INPUTS)
    ):
        failures.append("A4 did not distinguish an absent lint task from incomplete inputs")

    # Moon emitting no `inputFiles` for the task must FIRE, never silently skip: a skip would turn a
    # moon-version change into a vacuous pass, which is the failure mode this whole gate exists for.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["task_inputs"]["lint"] = None
    if not any(
        "inputFiles" in row
        for row in check_task_inputs(broken, crates, "lint", WORKSPACE_LINT_INPUTS)
    ):
        failures.append("A4 did not fire when moon reported no inputFiles")

    # A5 (SMA-546): the FFI build tasks must key on the workspace files. Fixture mirrors the real
    # shape — a ts project whose `build` shells out to napi + wasm-pack.
    ffi_ok = {
        "paigasus-kernel-ts": {
            "source_dir": "ts/packages/paigasus-kernel",
            "deps": {},
            "tasks": {"build": [], "test": []},
            "task_inputs": {
                "build": list(FFI_TASK_INPUTS),
                "test": list(FFI_TASK_INPUTS),
            },
            "invocations": {
                "build": "touch ... && napi build --platform && wasm-pack build .",
                "test": "touch ... && napi build --platform && wasm-pack build .",
            },
        },
        "paigasus-kernel-py": {
            "source_dir": "py/packages/paigasus-kernel",
            "deps": {},
            "tasks": {"test": []},
            "task_inputs": {"test": list(FFI_TASK_INPUTS)},
            "invocations": {"test": "uv sync --reinstall-package paigasus-py-bindings"},
        },
        "unrelated-ts": {
            "source_dir": "ts/packages/unrelated",
            "deps": {},
            "tasks": {"build": []},
            "task_inputs": {"build": []},
            "invocations": {"build": "tsc --noEmit"},
        },
    }

    if check_ffi_inputs(ffi_ok):
        failures.append("A5 reported violations on the clean fixture")

    # Fires when a matched task omits one of the required files.
    broken = json.loads(json.dumps(ffi_ok))
    broken["paigasus-kernel-ts"]["task_inputs"]["build"] = list(WORKSPACE_LINT_INPUTS)
    rows = check_ffi_inputs(broken)
    if not rows:
        failures.append("A5 did not fire on a missing FFI workspace input")
    elif not any(".prototools" in row for row in rows):
        failures.append("A5 fired but did not name the missing file")

    # Moon emitting no `inputFiles` for a MATCHED FFI task must FIRE, never silently skip.
    broken = json.loads(json.dumps(ffi_ok))
    broken["paigasus-kernel-ts"]["task_inputs"]["build"] = None
    if not any("inputFiles" in row for row in check_ffi_inputs(broken)):
        failures.append("A5 did not fire when moon reported no inputFiles")

    # THE ANTI-VACUITY ROW. Neuter the marker match (as a package.json indirection or a renamed
    # uv flag would) and A5's derived set empties. Without the floor this reports PASS while
    # asserting nothing — the exact silent-degradation mode the floor exists to stop.
    broken = json.loads(json.dumps(ffi_ok))
    for task in ("build", "test"):
        broken["paigasus-kernel-ts"]["invocations"][task] = "pnpm run build:native"
    rows = check_ffi_inputs(broken)
    if not any("not matched by any FFI marker" in row for row in rows):
        failures.append("A5 did not fire when a required FFI task stopped matching the markers")

    # A task exposing NEITHER a command NOR a script is moon telling us nothing — infra (rc 2),
    # not an assertion failure. Mirrors A4's absent-inputFiles rule.
    broken = json.loads(json.dumps(ffi_ok))
    broken["paigasus-kernel-ts"]["invocations"]["build"] = None
    try:
        check_ffi_inputs(broken)
    except MoonOutputError:
        pass
    else:
        failures.append("A5 did not raise infra on a task with no command and no script")

    # A matched task that is NOT in the floor is still asserted — this is the derived half.
    broken = json.loads(json.dumps(ffi_ok))
    broken["unrelated-ts"]["invocations"]["build"] = "wasm-pack build ."
    if not any("unrelated-ts:build" in row for row in check_ffi_inputs(broken)):
        failures.append("A5 did not assert a newly-matched task outside the floor")

    # A6 (SMA-528): every crate's build/test/lint must key on its TRANSITIVE upstreams' sources.
    # Independent of A1-A5 again — and uniquely unprotected elsewhere, because a crate's own
    # moon.yml is not an input to its own tasks, so a wrong `fileGroups.upstreams` reds nothing.
    #
    # EVERY control below matches a row KIND, never mere non-emptiness. A6 emits five distinct
    # kinds and most mutations trip several at once, so a bare `if not check_upstream_inputs(...)`
    # passes on a row from a DIFFERENT sub-assertion than the one it claims to prove. Measured, not
    # theorised: with the floor's derivation rows deleted, the first draft of these controls still
    # printed `all six assertions fire` and exited 0 — emptying `deps` also empties `want`, so the
    # per-crate loop supplied six `inputs include ...` rows and the floor control read them as its
    # own. Same discipline as A5's `unrelated-ts:build` case.
    #
    # A6 clean baseline — and it is a REAL control, not a smoke test. A6-a/A6-b below prove a row
    # appears when the DECLARATION lacks an entry; only this proves no row appears when it has one.
    # Neither alone pins a bucket: drop the `inputFiles` half of `resolved` entirely and A6-b still
    # passes (its Cargo.toml row shows up for the wrong reason) while THIS reds, because the clean
    # fixture declares that manifest in `inputFiles`. The pair is what holds both buckets wired.
    rows = check_upstream_inputs(ok, allow={}, floor={})
    if rows:
        failures.append(f"clean fixture reported A6 violations: {rows}")

    # A6-a: the GLOB half missing. Must name the upstream SRC entry on the task that lost it.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["task_input_globs"]["test"] = []
    if "a-rs:test inputs omit rs/crates/libs/b/src/**/*" not in check_upstream_inputs(
        broken, allow={}, floor={}
    ):
        failures.append("A6 did not fire on a missing upstream src glob")

    # A6-b: the MANIFEST half missing. This is the half an inputGlobs-only A6 could never see —
    # the exact defect the pre-review draft of the SMA-528 spec shipped. Naming the entry pins
    # WHICH row fired, so the control cannot be satisfied by an unrelated one. Note what it still
    # cannot do alone: dropping the `inputFiles` bucket from `resolved` produces this same row, so
    # A6-b passes and it is the CLEAN BASELINE above that reds. Do not weaken either one.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["task_inputs"]["build"] = []
    if "a-rs:build inputs omit rs/crates/libs/b/Cargo.toml" not in check_upstream_inputs(
        broken, allow={}, floor={}
    ):
        failures.append("A6 did not fire on a missing upstream Cargo.toml")

    # A6-c: over-approximation with no allowlist entry — the direction a subset check cannot see.
    ghost = "rs/crates/libs/ghost/src/**/*"
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["task_input_globs"]["lint"].append(ghost)
    if f"a-rs:lint inputs include {ghost}, which is not in its closure" not in (
        check_upstream_inputs(broken, allow={}, floor={})
    ):
        failures.append("A6 did not fire on an upstream outside the closure")

    # A6-c2: a BROAD glob into a crate that IS a real upstream — the hole the `observed` filter
    # was widened to close (SMA-528 CodeRabbit finding). Unlike A6-c's `ghost` entry, `b-rs` IS in
    # `a-rs`'s closure — but a `**/*` glob is still not the `src/**/*`-or-`Cargo.toml` pair `want`
    # expects, so strict equality must still fire on the literal mismatch. Before the widened
    # filter this entry matched neither suffix, so it never entered `observed` and the violation
    # was invisible regardless of what the fixture declared.
    broad = "rs/crates/libs/b/**/*"
    broad_broken = json.loads(json.dumps(ok))
    broad_broken["a-rs"]["task_input_globs"]["lint"].append(broad)
    if f"a-rs:lint inputs include {broad}, which is not in its closure" not in (
        check_upstream_inputs(broad_broken, allow={}, floor={})
    ):
        failures.append("A6 did not fire on a broad glob into a real upstream's tree")

    # A6-d: an allowlisted over-approximation must be accepted, and only WITH a reason. `broken`
    # carries exactly one defect, so a working allowlist empties the list outright.
    if check_upstream_inputs(
        broken, allow={("a-rs", "rs/crates/libs/ghost"): "deliberate"}, floor={}
    ):
        failures.append("A6 rejected an allowlisted over-approximation")
    if not any(
        ghost in row
        for row in check_upstream_inputs(
            broken, allow={("a-rs", "rs/crates/libs/ghost"): ""}, floor={}
        )
    ):
        failures.append("A6 accepted an allowlist entry with an empty reason")

    # A6-e: the FLOOR must fire when the derivation degrades to empty. Emptying `deps` also empties
    # `want`, which makes the per-crate loop emit six `inputs include ...` rows all by itself — so
    # this MUST match the `FLOOR:` prefix, or it passes with the entire floor block deleted.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["deps"] = {}
    if not any(
        row.startswith("FLOOR:")
        for row in check_upstream_inputs(broken, allow={}, floor={"a-rs": {"b-rs"}})
    ):
        failures.append("A6 floor did not fire on a neutered closure derivation")

    # The floor's OTHER half: a crate that stops reporting `language: rust` drops out of the
    # per-crate loop entirely. Its closure still derives, so the derivation half of the floor is
    # blind to it and the whole run goes green with that crate asserted about nothing.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["language"] = "unknown"
    if not any(
        "A6 examines" in row
        for row in check_upstream_inputs(broken, allow={}, floor={"a-rs": {"b-rs"}})
    ):
        failures.append("A6 floor did not fire on a crate that dropped out of the examined set")

    # A6-f: an absent inputGlobs key is an infra-shaped violation, never a silent skip.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["task_input_globs"]["build"] = None
    if not any(
        "a-rs:build reported no inputFiles/inputGlobs" in row
        for row in check_upstream_inputs(broken, allow={}, floor={})
    ):
        failures.append("A6 did not fire on an absent inputGlobs key")

    # ...and so is ONE bucket missing the task key while the other has it. Unreachable from
    # moon_projects() today, which writes both keys together. With a STRING sentinel this fell
    # through to `set("absent") | set(globs)`: the single letters are filtered out by the
    # `rs/crates/` prefix test, so the visible symptom is not junk rows but a CONFIDENT WRONG one —
    # `inputs omit .../Cargo.toml`, blaming the declaration for moon's half-report. Both halves are
    # asserted: the honest row present, the misleading row absent.
    broken = json.loads(json.dumps(ok))
    del broken["a-rs"]["task_input_globs"]["build"]
    rows = check_upstream_inputs(broken, allow={}, floor={})
    if not any("a-rs:build reported no inputFiles/inputGlobs" in row for row in rows):
        failures.append("A6 did not fire on a half-reported task (one bucket missing its key)")
    if any(row.startswith("a-rs:build inputs omit") for row in rows):
        failures.append("A6 blamed the declaration for a half-reported task")

    # A CYCLE must not make a crate its own upstream. `rust_closure` excludes the ORIGIN at every
    # depth; comparing against the current node instead would return {b-rs, a-rs} here, and `want`
    # would demand a-rs's own pair that `observed` deliberately excludes — an unsatisfiable row.
    broken = json.loads(json.dumps(ok))
    broken["b-rs"]["deps"] = {"a-rs": "explicit"}
    if any(
        row.startswith("a-rs:") and "rs/crates/libs/a/" in row
        for row in check_upstream_inputs(broken, allow={}, floor={})
    ):
        failures.append("A6 made a crate its own upstream on a dependency cycle")

    # A malformed Cargo.toml must surface as INFRA (rc 2), not as an assertion failure. Exercise the
    # whole chain on a throwaway workspace — parser raises, cargo_crates propagates, INFRA_ERRORS
    # catches — so narrowing that tuple fails here instead of silently relabelling a broken manifest
    # as a graph regression. Done without Moon on purpose: `moon query projects` parses Cargo.toml
    # too and fails first, which masks this path end-to-end.
    with tempfile.TemporaryDirectory() as tmp:
        bad = Path(tmp) / "rs" / "crates" / "bad"
        bad.mkdir(parents=True)
        (bad / "Cargo.toml").write_text("[package\nname = ")
        try:
            cargo_crates(Path(tmp))
        except INFRA_ERRORS:
            pass
        else:
            failures.append("a malformed Cargo.toml did not raise out of cargo_crates()")

    # SMA-599 — script_cargo_lines under THE CONSERVATIVE RULE (see the constants block).
    # Every row below is either an exclusion the shell provably never executes (heredoc body,
    # comment tail, arithmetic) or a shape MEASURED against real bash to actually run cargo.
    # The five "must report" shapes are what the four-layer lexer this replaced got wrong.
    with tempfile.TemporaryDirectory() as tmp:

        def _lines(body):
            p = Path(tmp) / "probe.sh"
            p.write_text(body)
            return script_cargo_lines(p)

        def _reports(body, prefix="cargo build"):
            return any(
                r.segment.strip().startswith(prefix) and r.resolves and not r.locked
                for r in _lines(body)
            )

        # Comment cut is quote-aware: a `#` inside a string is not a comment marker, so
        # cutting there would delete a real invocation — the false-negative direction.
        if not _reports('echo "a # b" && cargo build\n'):
            failures.append(
                "script_cargo_lines cut at a `#` inside a string and lost `cargo build`"
            )

        # Backslash continuation: the flag is on the NEXT physical line, and the row is
        # reported against the FIRST.
        rows = _lines("cargo build \\\n  --locked\n")
        if len(rows) != 1 or not rows[0].locked or rows[0].lineno != 1:
            failures.append(
                f"script_cargo_lines did not join a backslash continuation: {rows}"
            )

        # Command scoping: --locked belongs to the SECOND command, not the first.
        if not _reports("cargo build && cargo metadata --locked\n"):
            failures.append(
                "script_cargo_lines let a later command's --locked cover `cargo build`"
            )

        # --no-deps does not resolve (MEASURED: it never rewrites the lock).
        rows = _lines("cargo metadata --format-version 1 --no-deps\n")
        if not rows or rows[0].resolves:
            failures.append(
                "script_cargo_lines treated `cargo metadata --no-deps` as resolving"
            )

        # ONE ROW PER INVOCATION (SMA-599 review). A segment holding two cargo calls must
        # emit two rows. `search` emitted one — the first, locked — and the nested unlocked
        # `cargo test` VANISHED, a silent false negative in the one direction this design
        # claims it cannot have. `ci/actionlint/run.sh:3715` is the live instance.
        rows = _lines('cargo build --locked --features "$(cargo test)"\n')
        if len(rows) != 2 or rows[0].locked is not True or rows[1].locked is not False:
            failures.append(
                f"script_cargo_lines did not emit one row per cargo invocation in a segment "
                f"holding two — the nested unlocked call is invisible: {rows}"
            )

        # The flag scope is bounded by the NEXT invocation, not by the end of the segment: a
        # nested call's --locked must not cover the outer one. Without the bound this reads
        # `cargo build` as locked and reports nothing.
        rows = _lines('cargo build "$(cargo test --locked)"\n')
        if len(rows) != 2 or rows[0].locked is not False:
            failures.append(
                f"script_cargo_lines let a NESTED invocation's --locked cover the outer "
                f"`cargo build`: {rows}"
            )

        # The --no-deps carve-out is bounded the same way. A `--no-deps` on a neighbouring
        # invocation must not excuse a resolving one in the same segment.
        rows = _lines('cargo build "$(cargo metadata --no-deps)"\n')
        if len(rows) != 2 or rows[0].resolves is not True or rows[1].resolves is not False:
            failures.append(
                f"script_cargo_lines applied a neighbouring `--no-deps` to `cargo build`, "
                f"which does resolve: {rows}"
            )

        # A cargo verb inside a heredoc BODY is data the shell never executes.
        if _lines("cat <<'PY'\ncargo build\nPY\n"):
            failures.append("script_cargo_lines read a cargo line inside a heredoc body")

        # An unterminated heredoc would silently swallow the rest of the file — infra abort.
        try:
            _lines("cat <<'PY'\ncargo build\n")
        except MoonOutputError:
            pass
        else:
            failures.append("script_cargo_lines did not raise on an unterminated heredoc")

        # Prose in a full-line comment is not an invocation.
        if _lines("# run cargo build here\n"):
            failures.append("script_cargo_lines reported a cargo verb inside a comment")

        # --- shapes MEASURED to run cargo unlocked in real bash; all must report ----------

        # The repo's own house idiom: a command substitution inside a double-quoted value.
        if not _reports(
            'VERSION="$(cargo metadata --format-version 1 | jq -r .version)"\n',
            "VERSION=",
        ):
            failures.append(
                "script_cargo_lines dropped `cargo metadata` inside a command substitution"
            )
        if not _reports('if ! OUT="$(cargo build 2>&1)"; then\n', "if ! OUT="):
            failures.append(
                "script_cargo_lines dropped/mis-flagged `cargo build` inside `$(...)`"
            )

        # `X="abc` / `--locked" cargo build` is ONE bash statement across two physical lines.
        # The `--locked` is string content sitting BEFORE the verb, so it locks nothing —
        # which is why the flag test is scoped to the segment tail AFTER the verb.
        if not _reports('X="abc\n--locked" cargo build\n', "--locked"):
            failures.append(
                "script_cargo_lines let a quoted string's leading `--locked` cover an "
                "unlocked `cargo build`"
            )

        # A substitution body spanning physical lines, and one closing on the line it opens
        # while its enclosing quote stays open. Both run cargo.
        if not _reports('X="$(\ncargo build\n)"\n'):
            failures.append(
                "script_cargo_lines lost `cargo build` inside a multi-line `$(...)`"
            )
        if not _reports('X="$(cargo build) more\nstuff"\n', "X="):
            failures.append(
                "script_cargo_lines lost `cargo build` inside a same-line `$(...)` whose "
                "enclosing quote stays open"
            )

        # THE DEFECT THAT RETIRED THE PREVIOUS DESIGN: `bash -c \` + newline + `"cargo build"`.
        # The old lexer decided exec-vs-plain from the RAW physical line while continuation
        # joining happened later on the LOGICAL line, so this reported zero rows and no error.
        if not _reports('bash -c \\\n  "cargo build"\n', "bash -c"):
            failures.append(
                "script_cargo_lines dropped a `bash -c` invocation split across a "
                "backslash continuation"
            )

        # `$((1 << BITS))` is arithmetic, not a heredoc named BITS. Misread as one, it would
        # swallow every line up to the next bare `BITS`, a real unlocked cargo call included.
        if not _reports("MASK=$((1 << BITS))\ncargo build\nBITS\n"):
            failures.append(
                "script_cargo_lines misread `$((1 << BITS))` as a heredoc opener"
            )

        # THE ACCEPTED FALSE POSITIVE, pinned deliberately. A benign multi-line string that
        # merely mentions a cargo verb REPORTS. That is the cost of not stripping strings,
        # and it is the loud direction: CI reds and a reviewer adds a waiver. The previous
        # design suppressed this case and paid for it with three silent false negatives.
        if not _reports('echo "start\ncargo build\nend"\n'):
            failures.append(
                "script_cargo_lines suppressed a cargo verb inside a multi-line string — "
                "the conservative rule reports it deliberately"
            )

        # --- `_line_regions`: the two line-local exclusions, one decision each ------------
        #
        # Both can DROP a live invocation, which is the silent-pass direction. Every row below
        # was executed under real bash with cargo stubbed before it was written down.
        #
        # 1a. Ambiguous parity refuses the comment cut. An ODD count of surviving quotes means
        # the mask paired the wrong characters, so the `#` it found may be string content.
        # `X="a` newline `b # c" cargo build` is one bash statement that runs cargo.
        if not _reports('X="a\nb # c" cargo build\n', "b # c"):
            failures.append(
                "script_cargo_lines cut at a `#` on a line with unbalanced quotes and lost "
                "`cargo build`"
            )
        # 1b. The SAME ambiguity refuses to open a heredoc. Without that, the refused comment
        # cut leaves a `<<EOF` standing inside what is really string content, the body is
        # skipped, and the invocation vanishes with no error.
        if not _reports('echo \\" # <<EOF\ncargo build\nEOF\ncargo build --locked\n'):
            failures.append(
                "script_cargo_lines opened a heredoc from a line whose quote parity is "
                "ambiguous and swallowed `cargo build`"
            )
        # 1c. The cut runs PER PHYSICAL LINE, before continuations are joined. A `#` comment
        # ends at the newline even when the previous line ends in a backslash, so joining
        # first would swallow this `cargo build` into the comment.
        if not _reports("# note \\\ncargo build\n"):
            failures.append(
                "script_cargo_lines joined a comment across a backslash continuation and "
                "lost `cargo build`"
            )
        # 2. The `#` must start a WORD. `${#arr[@]}` is bash's length operator, never a
        # comment; cutting there drops everything after it. This guard shipped in an earlier
        # round with no fixture, and its mutant passed at rc 0 (SMA-599 round 5).
        if not _reports("n=${#arr[@]} && cargo build\n"):
            failures.append(
                "script_cargo_lines read `${#arr[@]}` as a comment and lost `cargo build`"
            )
        # 3. A heredoc opener must be UNMASKED — a `<<EOF` inside a quoted string is text, not
        # an opener, and treating it as one skips real code as if it were a body. Five shapes,
        # all measured: bare, `<<-` with a tab-indented terminator, a quoted delimiter inside
        # the string, an opener inside a SINGLE-quoted string, and the realistic prose case.
        for label, body in (
            ("a bare opener", 'echo "a <<EOF b"\ncargo build\nEOF\ncargo build --locked\n'),
            ("`<<-`", 'echo "a <<-EOF b"\ncargo build\n\tEOF\ncargo build --locked\n'),
            ("a quoted delimiter", "echo \"a <<'EOF' b\"\ncargo build\nEOF\ncargo build --locked\n"),
            ("a single-quoted string", "echo 'a <<EOF b'\ncargo build\nEOF\ncargo build --locked\n"),
            ("prose", 'die_infra "run <<EOF to reproduce"\ncargo build\nEOF\ncargo build --locked\n'),
        ):
            if not _reports(body):
                failures.append(
                    f"script_cargo_lines opened a phantom heredoc from {label} inside a "
                    f"string and swallowed `cargo build`"
                )
        # 3a2. A `<<EOF` in a COMMENT is not an opener either. Two independent mechanisms
        # stop it — the scan runs on the comment-cut code region, and the mask-position check
        # rejects any offset past that region — so no single mutation can break it. The row
        # exists to pin the behaviour, not because one mechanism is load-bearing alone.
        if not _reports("# see <<EOF\ncargo build\nEOF\ncargo build --locked\n"):
            failures.append(
                "script_cargo_lines opened a heredoc from a `<<` inside a comment and "
                "swallowed `cargo build`"
            )
        # 3b. POSITIVE CONTROL for the same rule: a REAL heredoc must still open, quoted
        # delimiter and redirection included, and its body must still be skipped. Without
        # this, "never open a heredoc" would pass every row above.
        rows = _lines('cat <<\'EOF\' > "$out"\ncargo build\nEOF\ncargo build\n')
        if len(rows) != 1 or rows[0].lineno != 4:
            failures.append(
                f"script_cargo_lines did not skip the body of a real `cat <<'EOF' > \"$out\"` "
                f"heredoc: {rows}"
            )
        # 3c. The ambiguity test counts SINGLE quotes too, because `X='a` newline
        # `b <<EOF c'` puts the opener inside a single-quoted span that bash never reads as
        # a heredoc. Only the heredoc decision counts them (see `_odd_quotes`).
        if not _reports("X='a\nb <<EOF c'\ncargo build\nEOF\n"):
            failures.append(
                "script_cargo_lines opened a heredoc from a `<<` inside a cross-line "
                "single-quoted span and swallowed `cargo build`"
            )
        # 3d. ...and the COMMENT cut does NOT count them, because an apostrophe in prose is
        # English. Counting singles there refuses the cut, leaves the `<<EOF` standing in the
        # scanned region, and stops this REAL heredoc from opening.
        rows = _lines("cat <<EOF # don't\ncargo build\nEOF\ncargo build\n")
        if len(rows) != 1 or rows[0].lineno != 4:
            failures.append(
                f"script_cargo_lines let an apostrophe in a comment stop a real heredoc from "
                f"opening: {rows}"
            )

        # --- round 6: the same masking defect, applied to `<<`'s four siblings ------------
        #
        # 4. Operator spans are blanked IN THE MASK, not in the code, and only when they
        # CLOSE. The old form ran on the raw line before any quote mask and blanked from an
        # unclosed `$((` to end of line, deleting the invocation from the code itself.
        if not _reports("echo '$(( x' && cargo build\n"):
            failures.append(
                "script_cargo_lines blanked the rest of the line from an unclosed `$((` "
                "inside a string and lost `cargo build`"
            )
        if not _reports("echo '$((' ; cargo build ; echo '))'\n"):
            failures.append(
                "script_cargo_lines paired a `$((` and a `))` that are both string content "
                "and lost `cargo build`"
            )
        # 5. A shift lives in three span shapes, not one: `$(( ))`, a bare `(( ))` arithmetic
        # command, and an array subscript `name[ ]`. Each hides a `<<` that is not an opener.
        # The subscript rule needs the word character before the `[` — `[ -f x ]` is a test
        # command and must not be blanked.
        if not _reports("(( MASK = 1 << BITS ))\ncargo build\nBITS\n"):
            failures.append(
                "script_cargo_lines read the shift in a bare `(( ... ))` as a heredoc opener "
                "and swallowed `cargo build`"
            )
        if not _reports("a[1 << N]=2\ncargo build\nN\n"):
            failures.append(
                "script_cargo_lines read the shift in an array subscript as a heredoc opener "
                "and swallowed `cargo build`"
            )
        # 6. `<<<` is a here-STRING and opens no body. HEREDOC_OPEN_RE matches at the SECOND
        # `<`, where the mask check passes, so the third one is rejected explicitly.
        if not _reports("cat <<<EOF\ncargo build\nEOF\n"):
            failures.append(
                "script_cargo_lines treated a `<<<` here-string as a heredoc opener and "
                "swallowed `cargo build`"
            )
        # 7. A heredoc body starts after the whole LOGICAL line. `cat <<EOF \` + newline +
        # `| cargo build` is one command; ending the line at the opener made the continuation
        # its body.
        if not _reports("cat <<EOF \\\n| cargo build\nEOF\ncargo build --locked\n"):
            failures.append(
                "script_cargo_lines treated the continuation of a heredoc-opener line as "
                "body and swallowed `cargo build`"
            )
        # 8. An ESCAPED space is not a word boundary: `echo a\ #b` keeps `#b` inside the word,
        # so nothing is a comment and the `&&` invocation runs.
        if not _reports("echo a\\ #b && cargo build\n"):
            failures.append(
                "script_cargo_lines cut at a `#` behind an escaped space and lost "
                "`cargo build`"
            )
        # 9. POSITIVE CONTROLS for rows 5-7: the two remaining real opener shapes must still
        # open and still skip their bodies. Without these, "never open a heredoc" passes every
        # negative row above.
        for label, probe in (
            ("plain `cat <<EOF`", "cat <<EOF\ncargo build\nEOF\ncargo build\n"),
            ("`cat <<-EOF`", "cat <<-EOF\ncargo build\n\tEOF\ncargo build\n"),
            # An UNCLOSED bracketed span must be left alone, not blanked to end of line:
            # `ls a[bc <<EOF` is a glob word followed by a REAL heredoc, and blanking from the
            # `[` onwards hides the opener, so the body gets scanned as code. This is the row
            # that makes the closed-span requirement testable — every other operator-span
            # fixture passes with or without it.
            ("an unclosed `[` before a real opener",
             "ls a[bc <<EOF\ncargo build\nEOF\ncargo build\n"),
        ):
            rows = _lines(probe)
            if len(rows) != 1 or rows[0].lineno != 4:
                failures.append(
                    f"script_cargo_lines did not skip the body of a real {label} "
                    f"heredoc: {rows}"
                )

    # SMA-599 — derive_cargo_tasks must keep the three kinds DISTINGUISHABLE at the
    # derivation boundary. A8 measured that a wrapper match and a literal match cannot be
    # treated alike (paigasus-kernel-ts:build carries a --locked belonging to a DIFFERENT
    # command), so a flat set would silently reintroduce the vacuous form.
    with tempfile.TemporaryDirectory() as tmp:
        ci_dir = Path(tmp) / "ci" / "probe"
        ci_dir.mkdir(parents=True)
        (ci_dir / "run.sh").write_text("cd rs\ncargo build\n")
        kinds_fixture = {
            "p": {
                "source_dir": ".", "deps": {}, "tasks": {},
                "task_inputs": {}, "task_input_globs": {},
                "invocations": {
                    "lit": "cargo build --locked",
                    "wrap": "pnpm exec napi build --platform",
                    "scr": "bash ci/probe/run.sh --negative-control",
                    "none": "echo hello",
                },
            },
        }
        got = derive_cargo_tasks(kinds_fixture, Path(tmp))
        want = {"p:lit": "literal", "p:wrap": "wrapper", "p:scr": "script"}
        if got != want:
            failures.append(f"derive_cargo_tasks returned {got}, expected {want}")

        # An unresolvable script path must ABORT, not silently shrink the derived set.
        missing = json.loads(json.dumps(kinds_fixture))
        missing["p"]["invocations"]["scr"] = "bash ci/gone/run.sh"
        try:
            derive_cargo_tasks(missing, Path(tmp))
        except MoonOutputError:
            pass
        else:
            failures.append("derive_cargo_tasks did not raise on a script path that does not exist")

    # Precedence: wrapper > literal for a task matching BOTH kinds. All four fixture tasks
    # above carry a single signal, so this is the only row that fails if the if/elif branches
    # are swapped — A8 measured that collapsing a wrapper match to literal is vacuous
    # (paigasus-kernel-ts:build's `napi build` is unlocked beside a locked `wasm-pack build`).
    both = {
        "p": {
            "source_dir": ".", "deps": {}, "tasks": {},
            "task_inputs": {}, "task_input_globs": {},
            "invocations": {"mixed": "pnpm exec napi build --platform && cargo build --locked"},
        },
    }
    if derive_cargo_tasks(both, Path(".")) != {"p:mixed": "wrapper"}:
        failures.append(
            "derive_cargo_tasks did not apply wrapper > literal precedence to a task matching "
            "BOTH kinds — the stricter rule must win"
        )

    # SMA-605 — the BLOB arm. Deliberately blob-only fixtures with NO script reference: every
    # other indirect fixture reaches the code through a script, so without these, deleting the
    # blob wiring survives the whole suite at rc 0 (SMA-605 review).
    def _blob(cmd):
        return {
            "p": {
                "source_dir": "rs/crates/libs/p", "deps": {}, "tasks": {},
                "task_inputs": {"t": []}, "task_input_globs": {"t": []},
                "invocations": {"t": cmd},
            },
        }

    for cmd, want_kind in (
        ('"$CARGO_BIN" build', "literal"),
        ("CARGO=/p release-plz update", "wrapper"),
    ):
        got = derive_cargo_tasks(_blob(cmd), Path("."))
        if got != {"p:t": want_kind}:
            failures.append(
                f"derive_cargo_tasks did not classify the blob {cmd!r} as {want_kind} — it "
                f"returned {got}; the blob arm is not wired"
            )

    # ...and A8 must actually REPORT them, not merely derive them.
    if not any(
        "p:t" in r for r in check_cargo_locked(_blob('"$CARGO_BIN" build'), allow={}, floor=())
    ):
        failures.append("A8's blob arm did not report an unlocked indirect cargo invocation")
    # Arm 2 in a blob is a WRAPPER: a --locked in the blob must NOT clear it.
    if not any(
        "p:t" in r
        for r in check_cargo_locked(
            _blob("CARGO=/p release-plz update --locked"), allow={}, floor=()
        )
    ):
        failures.append(
            "A8's blob arm let a --locked clear a CARGO= redirection — the flag reaches the "
            "tool, never the cargo behind it"
        )
    # The row must name the CAUSE it actually matched. `is_wrapper` covers two shapes, and a
    # "(FFI_MARKERS)" row for a `CARGO=` blob sends the reviewer looking for a napi/wasm-pack
    # call that does not exist (CodeRabbit PR review).
    env_rows = check_cargo_locked(_blob("CARGO=/p release-plz update"), allow={}, floor=())
    if not any("CARGO= redirection" in r for r in env_rows):
        failures.append(f"A8's blob row does not name the CARGO= cause: {env_rows}")
    if any("FFI_MARKERS" in r for r in env_rows):
        failures.append(
            f"A8's blob row blames FFI_MARKERS for a CARGO= redirection it never matched: "
            f"{env_rows}"
        )
    # The blob arm must use MERGED-match semantics, exactly like the script arm: an env prefix
    # whose command IS cargo is one locked literal call, not a wrapper needing a waiver. Searching
    # the raw env regex made the two arms disagree about one string (CodeRabbit PR review).
    locked_env = _blob("CARGO=/p cargo build --locked")
    if derive_cargo_tasks(locked_env, Path(".")) != {"p:t": "literal"}:
        failures.append(
            "derive_cargo_tasks classified `CARGO=/p cargo build --locked` as a wrapper — the "
            "blob arm bypasses cargo_matches' env suppression"
        )
    if check_cargo_locked(locked_env, allow={}, floor=()):
        failures.append(
            "A8's blob arm demanded a waiver for `CARGO=/p cargo build --locked`, a correctly "
            "locked literal call"
        )
    ffi_rows = check_cargo_locked(_blob("pnpm exec napi build --platform"), allow={}, floor=())
    if not any("FFI_MARKERS" in r for r in ffi_rows):
        failures.append(f"A8's blob row lost the FFI_MARKERS cause: {ffi_rows}")

    # SMA-605 — the merged match list. Both arms are FORWARD COVER: arm 1 reports zero rows on
    # the real corpus and arm 2 exactly one, and only once the source resolver lands. These
    # fixtures are the whole proof that either arm works.
    def _kinds(text):
        return [(c.kind, c.verb) for c in cargo_matches(text)]

    for text, want in (
        ('cargo build --locked', [("literal", "build")]),
        ('"$CARGO_BIN" build', [("var", "build")]),
        ('"${CARGO_BIN}" build', [("var", "build")]),
        ('CARGO=/p release-plz update', [("env", None)]),
        # Arm 1 must NOT fire on a variable whose name does not mention cargo. All three are
        # live lines in this repo, and a naive widening reports all three (SMA-605 M4).
        ('git -C "$dir" add -A', []),
        ('echo "negative control: $failures check(s) failed to bite"', []),
        ('"$RELEASE_PLZ_BIN" update', []),
        # Arm 2 is EXACTLY `CARGO=`. CARGO_NET_OFFLINE configures cargo; it does not redirect it.
        ('CARGO_NET_OFFLINE=true tool update', []),
        # ...and it must still see a real CARGO= that follows one.
        ('CARGO_NET_OFFLINE=true CARGO=/p tool update', [("env", None)]),
        # An assignment with nothing to run is not an invocation. NOTE: this fixture does NOT
        # distinguish the lookahead from a consuming `\\s+\\S` — both report zero here, measured.
        # The row below is the one that does.
        ('export CARGO=/p', []),
        # ...and the lookahead must not cross a NEWLINE to find one. Fifteen real moon blobs are
        # multi-line `script:` blocks, so a `\s`-based lookahead makes an unrelated next line
        # into a wrapper match (MEASURED, CodeRabbit local review).
        ("export CARGO=/p\necho hi", []),
        # ARM 1 MUST NOT SPAN A NEWLINE either (CodeRabbit PR review). COMMAND_SPLIT_RE does not
        # split on newlines and a blob is often a multi-line `script:` block, so a `\s` between
        # the variable and its verb reads two separate commands as one invocation.
        ('"$CARGO_BIN"\nbuild', []),
        # An env prefix followed only by MORE ASSIGNMENTS runs nothing, so it is not a wrapper...
        ("CARGO=/p CARGO_HOME=/x", []),
        # ...but one followed by assignments AND a command still is: the tool is reached with
        # cargo redirected, so skipping the assignments matters more than rejecting them.
        ("CARGO=/p CARGO_HOME=/x tool run", [("env", None)]),
        # An env prefix whose COMMAND IS CARGO is not indirection. Reporting both kinds gives two
        # rows with the SAME segment text, which makes every waiver for that line permanently
        # ambiguous — the line becomes unwaivable (SMA-599 L15, MEASURED on the locked form).
        ("CARGO=/p cargo build", [("literal", "build")]),
        ("CARGO=/p cargo build --locked", [("literal", "build")]),
        ("CARGO=/p CARGO_HOME=/x cargo build", [("literal", "build")]),
        # ...but a SEPARATE command after it keeps both: the redirection governs `tool`, not the
        # cargo call that follows the `&&`.
        ("CARGO=/p tool update && cargo build", [("env", None), ("literal", "build")]),
        # THE LOOKAHEAD'S PROOF, and the only shape that separates it from consumption
        # (measured over eight candidate shapes). Consuming the trailing word eats the second
        # prefix's leading separator, so `finditer` resumes mid-token and reports ONE match
        # where there are two.
        ('CARGO=/p CARGO=/q tool update', [("env", None), ("env", None)]),
        # A lowercase `$cargo build` is ALREADY matched by CARGO_INVOCATION_RE (measured), so
        # without de-duplication arm 1 double-reports one invocation.
        ('$cargo build', [("literal", "build")]),
    ):
        if _kinds(text) != want:
            failures.append(
                f"cargo_matches({text!r}) is {_kinds(text)}, expected {want}"
            )

    if not REQUIRED_LOCKED_TASKS:
        failures.append("REQUIRED_LOCKED_TASKS is empty — A8's floor would assert nothing")

    # SMA-599 — A10. Its verb predicate is its OWN, not LOCK_RESOLVING_VERBS: reusing A8's list
    # excluded the thirteen `fmt` tasks by COINCIDENCE and would hide any future
    # compiling-but-not-resolving subcommand (cargo llvm-cov, insta, udeps).
    a10_fixture = {
        "c-rs": {
            "source_dir": "rs/crates/libs/c", "deps": {}, "tasks": {},
            "task_inputs": {"build": ["rs/.cargo/config.toml"], "fmt": []},
            "task_input_globs": {"build": [], "fmt": []},
            "invocations": {"build": "cargo build --locked", "fmt": "cargo fmt --check"},
        },
        "repo": {
            "source_dir": ".", "deps": {}, "tasks": {},
            "task_inputs": {"deny": [], "tree": [], "mach": [], "root-build": []},
            "task_input_globs": {"deny": [], "tree": [], "mach": [], "root-build": []},
            "invocations": {
                "deny": "cargo deny --locked --manifest-path rs/Cargo.toml check",
                "tree": "cd rs && cargo tree --locked -p x",
                "mach": "cargo machete rs",
                # cwd-only exclusion coverage: verb-sensitive (`build`), cwd is the repo root.
                "root-build": "cargo build --manifest-path rs/Cargo.toml",
            },
        },
    }
    if check_cargo_config_inputs(a10_fixture, Path("."), allow={}, floor=("c-rs:build",)):
        failures.append("A10 reported violations on a clean fixture")

    # A10's cwd derivation, exercised directly. Substitution runs LONGEST NAME FIRST because
    # `str.replace` on the bare `$NAME` form has no word boundary: a short name that prefixes a
    # longer one eats it. MEASURED before the fix — `R=zzz` ahead of `RS_DIR="$REPO_ROOT/rs"`
    # rewrote `$RS_DIR` to `zzzS_DIR` and the same script resolved to False, while dropping the
    # `R=` line resolved to True. Dict order is the script's assignment order, so the verdict
    # depended on which variable a file happened to define first.
    for label, probe, want in (
        ("a bare `cd rs`", "cd rs\ncargo build\n", True),
        ("one round of substitution",
         'RS_DIR="$REPO_ROOT/rs"\ncd "$RS_DIR"\ncargo build\n', True),
        ("a prefix-colliding shorter name defined FIRST",
         'R=zzz\nRS_DIR="$REPO_ROOT/rs"\ncd "$R_DIR_UNUSED"\ncd "$RS_DIR"\ncargo build\n', True),
        ("a cwd outside rs/", 'cd ts\ncargo build --manifest-path rs/Cargo.toml\n', False),
    ):
        if _cwd_inside_rs(probe, ".") is not want:
            failures.append(
                f"_cwd_inside_rs returned {not want} for {label} — the cwd rule is wrong in "
                f"the {'false-negative' if want else 'false-positive'} direction"
            )

    # The core assertion: an in-scope task missing the input.
    broken = json.loads(json.dumps(a10_fixture))
    broken["c-rs"]["task_inputs"]["build"] = []
    if not any("c-rs:build" in r for r in
               check_cargo_config_inputs(broken, Path("."), allow={}, floor=("c-rs:build",))):
        failures.append("A10 did not fire on a cargo-from-rs task missing rs/.cargo/config.toml")

    # `fmt` never reaches A10's verb test at all: `fmt` is absent from LOCK_RESOLVING_VERBS, so
    # `derive_cargo_tasks` never derives `c-rs:fmt` in the first place. This row is a
    # KNOWN-VACUOUS forward guard, exactly like `repo:mach` below, not a live exercise of
    # CONFIG_SENSITIVE_VERBS (SMA-599 review corrected the prior "excluded BY THE VERB" claim,
    # which wrongly implied the verb test runs and rejects `fmt` — it never runs at all).
    if any("c-rs:fmt" in r for r in
           check_cargo_config_inputs(a10_fixture, Path("."), allow={}, floor=("c-rs:build",))):
        failures.append("A10 demanded rs/.cargo/config.toml from `cargo fmt`, which cannot read it")

    # `repo:deny` and `repo:mach` are excluded BY VERB, not cwd (SMA-599 review corrected the
    # prior comment here, "cwd is what excludes repo:deny", which was factually wrong). `deny`
    # IS in LOCK_RESOLVING_VERBS and does reach `derive_cargo_tasks`, but is not in
    # CONFIG_SENSITIVE_VERBS, so `sensitive` is False and `_cwd_inside_rs` is never even
    # called — this row exercises the verb split, exactly like `repo:tree` below, and proves
    # nothing about cwd or about `--manifest-path`/a bare `rs` argument. `machete` is absent
    # from LOCK_RESOLVING_VERBS entirely, so `repo:mach` never reaches `derive_cargo_tasks` at
    # all — a KNOWN-VACUOUS forward guard, like `c-rs:fmt` above. The actual cwd-only exclusion
    # (a verb-sensitive task whose cwd is NOT rs/) is `repo:root-build`, asserted below — before
    # that fixture existed, mutating `_cwd_inside_rs` to unconditionally `return True` survived
    # both --self-test and the real corpus.
    for task in ("repo:deny", "repo:mach"):
        if any(task in r for r in
               check_cargo_config_inputs(a10_fixture, Path("."), allow={}, floor=("c-rs:build",))):
            failures.append(f"A10 pulled {task} into scope from an `rs` path ARGUMENT, not a cd")

    # `cargo tree` runs from rs/ but resolves without compiling — out of scope by verb (AC 4).
    if any("repo:tree" in r for r in
           check_cargo_config_inputs(a10_fixture, Path("."), allow={}, floor=("c-rs:build",))):
        failures.append("A10 demanded the config file from `cargo tree`, which never compiles")

    # cwd-only exclusion, negative coverage (SMA-599 review). Unlike repo:deny/repo:mach above,
    # this task IS verb-sensitive (`build`) — its ONLY exclusion is cwd being the repo root, not
    # rs/, so this is the fixture that actually exercises `_cwd_inside_rs`'s False branch on a
    # sensitive task.
    if any("repo:root-build" in r for r in
           check_cargo_config_inputs(a10_fixture, Path("."), allow={}, floor=("c-rs:build",))):
        failures.append(
            "A10 pulled repo:root-build into scope though its cargo build runs from the repo "
            "root, not rs/ — the cwd exclusion is not being applied"
        )

    # Both input buckets matter, and an absent bucket is a violation, never a skip — the same
    # contract A4/A5/A6/A7 already assert (SMA-599 review: neither half was exercised for A10).
    glob_only = json.loads(json.dumps(a10_fixture))
    glob_only["c-rs"]["task_inputs"]["build"] = []
    glob_only["c-rs"]["task_input_globs"]["build"] = ["rs/.cargo/config.toml"]
    if check_cargo_config_inputs(glob_only, Path("."), allow={}, floor=("c-rs:build",)):
        failures.append(
            "A10 ignored rs/.cargo/config.toml declared only in task_input_globs — dropping "
            "`| set(globs)` from the production code must fail this"
        )

    missing_bucket = json.loads(json.dumps(a10_fixture))
    missing_bucket["c-rs"]["invocations"]["check"] = "cargo check --locked"
    # `check` is deliberately absent from BOTH task_inputs and task_input_globs, simulating
    # moon's output shape changing underneath this task.
    if not any(
        "c-rs:check" in r and "inputFiles" in r
        for r in check_cargo_config_inputs(
            missing_bucket, Path("."), allow={}, floor=("c-rs:build",)
        )
    ):
        failures.append(
            "A10 did not report a violation for a task with no inputFiles/inputGlobs bucket at "
            "all — treating a missing bucket as a skip instead of a violation must fail this"
        )

    # Floor: a member that leaves scope must fail, or the derivation could empty silently.
    if not any("FLOOR:" in r for r in
               check_cargo_config_inputs(a10_fixture, Path("."), allow={}, floor=("c-rs:nope",))):
        failures.append("A10's floor did not fire on a member outside the derived set")

    # Second vacuity mode, specific to default-deny: an allowlist that swallows a floor member.
    swallow = {"c-rs:build": "a reason"}
    if not any("FLOOR:" in r for r in
               check_cargo_config_inputs(broken, Path("."), allow=swallow, floor=("c-rs:build",))):
        failures.append("A10's floor let an allowlist entry cover a floor member")

    # An empty reason is itself a row.
    if not any("empty reason" in r for r in check_cargo_config_inputs(
            broken, Path("."), allow={"c-rs:build": " "}, floor=())):
        failures.append("A10 accepted an ALLOW_MISSING_CARGO_CONFIG entry with an empty reason")

    # A10 must follow a task's script for EVERY kind, not only `script` (SMA-599 review).
    # `derive_cargo_tasks` assigns `literal` on any cargo verb anywhere in the blob, PROSE
    # INCLUDED, so a benign `echo "running cargo check"` in a moon.yml block changed a gate's
    # kind and — while A10 read scripts only for kind `script` — silently took that gate out
    # of scope. Two gates run the SAME script here and differ only by that echo; both must be
    # in scope and both must be reported. A8's script arm never had this bug.
    with tempfile.TemporaryDirectory() as tmp:
        probe = Path(tmp) / "ci" / "foo"
        probe.mkdir(parents=True)
        (probe / "run.sh").write_text("cd rs\ncargo build --locked\n")
        echo_fixture = {
            "repo": {
                "source_dir": ".", "deps": {}, "tasks": {},
                "task_inputs": {"plain": [], "chatty": []},
                "task_input_globs": {"plain": [], "chatty": []},
                "invocations": {
                    "plain": "bash ci/foo/run.sh",
                    "chatty": 'echo "running cargo check"; bash ci/foo/run.sh',
                },
            },
        }
        rows = check_cargo_config_inputs(
            echo_fixture, Path(tmp), allow={}, floor=("repo:plain", "repo:chatty")
        )
        for task in ("repo:plain", "repo:chatty"):
            if not any(task in r and "FLOOR:" not in r for r in rows):
                failures.append(
                    f"A10 did not report {task}, which reaches a compiling cargo from rs/ "
                    f"through its script — a benign `echo` in the blob must not change scope"
                )

    # SMA-599 (CodeRabbit round 1) — a nested invocation must NOT inherit the enclosing
    # command's flag. Before _tail_end, `cargo test` here read the outer `--locked` from its
    # tail and reported nothing: a silent false negative, which is the one failure this
    # scanner exists to prevent. Both invocations are genuinely unlocked, so both must report.
    nested = _classify_shell_line(1, 'cargo build --features "$(cargo test)" --locked')
    if len(nested) != 2 or any(r.locked for r in nested):
        failures.append(
            f"a nested cargo call inherited the enclosing --locked: "
            f"{[(r.segment.strip()[:30], r.locked) for r in nested]}"
        )
    # The converse, so the bound is not simply 'ignore every flag': a nested call that carries
    # its OWN --locked inside the substitution must still read locked.
    own = _classify_shell_line(1, 'X="$(cargo build --locked)"')
    if len(own) != 1 or not own[0].locked:
        failures.append("a nested cargo call lost its OWN --locked to the substitution bound")

    # SMA-599 L11, pinned rather than left as prose (CodeRabbit round 1). A compiling cargo
    # PLUGIN is invisible to A10 because every arm of `cargo_matches` filters on a VERB LIST, and
    # A8's is LOCK_RESOLVING_VERBS. That exclusion is INTENTIONAL and still out of scope: SMA-605
    # closed L10 by adding two arms for INDIRECTION — a cargo-named variable and a `CARGO=`
    # prefix — WITHOUT widening the verb list, so L11's subcommand shape is untouched. It must
    # stay a tested decision rather than an accident, so this fixture fails the day the verb list
    # widens and nobody revisits L11.
    for plugin in ("cargo llvm-cov", "cargo insta test", "cargo udeps", "cargo bloat"):
        if _classify_shell_line(1, f"cd rs && {plugin}"):
            failures.append(
                f"{plugin!r} now matches a cargo_matches arm — A10 can see it, so spec L11 is "
                f"stale; revisit it rather than deleting this row"
            )

    # SMA-599 (CodeRabbit round 2) — a `cd` INSIDE a command substitution must still confer
    # scope. Stripping substitutions is what makes `cd "$(git rev-parse ...)/rs"` resolve, but
    # applied alone it deleted the `cd rs` from `X="$(cd rs && cargo build)"` and the task read
    # as running outside rs/: a silent false negative. Both shapes are pinned, because a fix for
    # either one alone breaks the other.
    for probe, want in (
        ('X="$(cd rs && cargo build)"', True),
        ('cd "$(git rev-parse --show-toplevel)/rs" && cargo build', True),
        ('X="$(cd ts && cargo build)"', False),
    ):
        if _cwd_inside_rs(probe, ".") is not want:
            failures.append(
                f"_cwd_inside_rs({probe!r}) is not {want} — a cd inside a command substitution "
                f"and one wrapping it need opposite handling, and both must hold"
            )

    if not REQUIRED_CARGO_CONFIG_TASKS:
        failures.append("REQUIRED_CARGO_CONFIG_TASKS is empty — A10's floor would assert nothing")
    # SMA-605 — A10's arms are its OWN, built from CONFIG_SENSITIVE_VERBS. Reusing arm 1
    # (LOCK_RESOLVING_VERBS) pulls `tree`, `deny` and `update` into A10's scope and NOTHING
    # reds — the accident SMA-599 D9 spent a round removing.
    def _a10(cmd):
        return {
            "q": {
                "source_dir": "rs/crates/libs/q", "deps": {}, "tasks": {},
                "task_inputs": {"t": []}, "task_input_globs": {"t": []},
                "invocations": {"t": cmd},
            },
        }

    if not any(
        "q:t" in r and CARGO_CONFIG_INPUT in r
        for r in check_cargo_config_inputs(_a10('"$CARGO_BIN" build'), Path("."), floor=())
    ):
        failures.append("A10 did not demand .cargo/config.toml for an indirect compiling call")
    if check_cargo_config_inputs(_a10('"$CARGO_BIN" tree'), Path("."), floor=()):
        failures.append(
            "A10 examined `\"$CARGO_BIN\" tree` — its arm is built from LOCK_RESOLVING_VERBS "
            "rather than CONFIG_SENSITIVE_VERBS (SMA-599 D9)"
        )
    if not any(
        "q:t" in r
        for r in check_cargo_config_inputs(_a10("CARGO=/p release-plz update"), Path("."), floor=())
    ):
        failures.append(
            "A10 did not treat a CARGO= redirection as sensitive — the tool's inner cargo may "
            "compile, and A10 cannot know that it does not"
        )
    # ...and the clause above is only LOAD-BEARING for a redirection that lives inside a FOLLOWED
    # SCRIPT. A blob-level `CARGO=` is already `wrapper` by derivation, so the `kind == "wrapper"`
    # branch covers it and dropping the clause survives (MEASURED). A script-level one derives as
    # `script`, and then this clause is the only thing that sees it — SMA-599 L13's shape, where a
    # wrapper hides one level down and CONFIG_SENSITIVE_RE cannot recognise it.
    with tempfile.TemporaryDirectory() as tmp:
        eco = Path(tmp) / "ci" / "probe"
        eco.mkdir(parents=True)
        (eco / "eco.sh").write_text("CARGO=/p release-plz update\n")
        hidden = {
            "q": {
                "source_dir": "rs/crates/libs/q", "deps": {}, "tasks": {},
                "task_inputs": {"t": []}, "task_input_globs": {"t": []},
                "invocations": {"t": "bash ci/probe/eco.sh"},
            },
        }
        if not any(
            "q:t" in r and CARGO_CONFIG_INPUT in r
            for r in check_cargo_config_inputs(hidden, Path(tmp), floor=())
        ):
            failures.append(
                "A10 missed a CARGO= redirection hiding inside a followed script — the task "
                "derives as `script`, so the wrapper branch does not cover it"
            )

    # A10's arm 1, exercised DIRECTLY — the way _cwd_inside_rs is. Going through
    # check_cargo_config_inputs cannot isolate it: a blob the arm rejects is not derived at all,
    # so A10 never examines it and the row is absent for the wrong reason. Only a direct call
    # separates "the arm said no" from "the derivation said no" (MEASURED: the newline row
    # survived as a mutation until this table existed).
    for probe, want in (
        ('"$CARGO_BIN" build', True),
        ('"$CARGO_BIN" +nightly build', True),
        # ...but never ACROSS a newline: COMMAND_SPLIT_RE does not split there, and a blob is
        # often a multi-line `script:` block.
        ('"$CARGO_BIN"\nbuild', False),
        ('"$CARGO_BIN" +nightly\nbuild', False),
        # A8's verb, not A10's: `tree` resolves the graph and never compiles (SMA-599 D9).
        ('"$CARGO_BIN" tree', False),
        ('"$CARGO_BIN" update', False),
        # The NAME is the whole test.
        ('"$RELEASE_PLZ_BIN" build', False),
    ):
        if _var_sensitive(probe) is not want:
            failures.append(
                f"_var_sensitive({probe!r}) is {not want} — A10's arm 1 is wrong in the "
                f"{'false-negative' if want else 'false-positive'} direction"
            )

    if not CONFIG_SENSITIVE_VERBS:
        failures.append("CONFIG_SENSITIVE_VERBS is empty — A10 would examine nothing")

    for f in failures:
        print(f"  FAIL {f}", file=sys.stderr)
    if failures:
        print("negative-control FAILED: the parity gate can pass vacuously", file=sys.stderr)
        return 1
    print("  OK   [parity] all ten assertions fire on synthetic violations")
    return 0


# SMA-560 I4 — the findings list's own floor. `collect_findings` is the ONLY place a check is
# invoked for the real run, so a check dropped from that list is never called; but "never called"
# was, until now, only LOUD for a check whose name appears nowhere else in the list. Three shapes
# were measured passing `--self-test` with a real assertion silently removed: deleting the `a5`
# tuple (the name `check_ffi_inputs` still appears, on the line that pre-computes `a5`), deleting
# either `check_task_inputs` tuple (the name still appears on the other one), and deleting the
# `a1`/`a2`/`a3` tuples (`check` does not even carry the `check_` prefix the name guard scans for).
# A name-based guard structurally cannot separate two call sites of the same function, so this pins
# the LIST: `collect_findings` returns `(key, rows, title)` triples and `self_test` asserts both the
# arity and the exact key sequence, the way ci_targets.py's SELF_SCHEDULED_GATES pins a membership
# rather than a bare count.
#
# Adding a check means adding its key here AND its tuple there, in the same order.
EXPECTED_FINDING_KEYS = ("a1", "a2", "a3", "a4-lint", "a4-fmt", "a5", "a6", "a7", "a8", "a9", "a10")


def collect_findings(projects, crates, root):
    """Every assertion's rows, as `(key, rows, title)`, in report order.

    ONE list, used for BOTH the pass/fail verdict and the report. Before SMA-542 the two were
    written separately, so a new check folded into one and not the other was a green no-op.
    That restructure is necessary but NOT sufficient on its own: what makes the list itself
    hard to shrink is `EXPECTED_FINDING_KEYS` above, asserted by `self_test`.

    Raises the INFRA_ERRORS members its checks raise (`MoonOutputError` from the FFI derivation),
    so `main` keeps them inside its try and maps them to rc 2.
    """
    a1, a2, a3 = check(projects, crates)
    a5 = check_ffi_inputs(projects)
    a8 = (
        check_cargo_locked(projects, root)
        + check_dockerfile_locked(root)
        + check_cargo_locked_scripts(projects, root)
        + check_version_lockstep_no_write(projects)
        # SMA-605. Joins A8's bucket rather than becoming an eleventh key: the resolver widens
        # what A8 scans, it is not a new assertion, so EXPECTED_FINDING_KEYS is unchanged.
        + check_sourced_scripts(root)
    )
    # SMA-594. Derived, never hand-listed, for the same reason `self_test`'s `complete_inputs` is:
    # these two hints named three files while the checks already demanded four, so a developer who
    # followed the advice verbatim was left with a still-red gate. `/`-prefixed because that is the
    # form the YAML `inputs` take (the checks compare the resolved, slash-free form).
    #
    # Do NOT extend this to the `a4-fmt` hint below. Every member of these two constants is
    # WORKSPACE-relative, which is what makes a blanket `/` prefix right. FMT_TASK_INPUTS is mixed:
    # `rs/rustfmt.toml` is workspace-relative but `Cargo.toml`, `src/**/*` and `tests/**/*` are
    # PROJECT-relative, so the same one-liner would emit `/Cargo.toml` and `/src/**/*` and send the
    # reader to paths that do not exist. That hint stays hand-listed on purpose.
    want_lint_inputs = ", ".join(f"/{f}" for f in WORKSPACE_LINT_INPUTS)
    want_ffi_inputs = ", ".join(f"/{f}" for f in FFI_TASK_INPUTS)
    findings = [
        ("a1", a1,
             "Cargo dep with NO Moon edge (under-builds — CI stays green while skipping work).\n"
             "    Fix: add the upstream to `dependsOn` in the consumer's moon.yml."),
        ("a2", a2,
             "Hand-declared Moon edge with NO Cargo backing (over-builds).\n"
             "    Fix: delete it, or add it to ALLOW_NO_CARGO_BACKING with a reason."),
        ("a3", a3,
             "Moon edge exists but the upstream's build is NOT scheduled — the affected-graph\n"
             "    guard CANNOT see this (SMA-429 F3).\n"
             "    Fix: for `build`/`test`, add '^:build' to the task's `deps` in the consumer's\n"
             "    moon.yml. For `lint` the dep is declared once for ALL crates in\n"
             "    .moon/tasks/rust.yml — restore it there, not per-crate (SMA-526)."),
        ("a4-lint", check_task_inputs(projects, crates, "lint", WORKSPACE_LINT_INPUTS),
             "`lint` does not key on the workspace-level files, so a dependency bump, a\n"
             "    [workspace.lints] edit or a toolchain drift schedules NOTHING for this crate\n"
             "    (SMA-534).\n"
             "    Fix: the inputs are declared once for ALL crates in .moon/tasks/rust.yml —\n"
             "    restore them there, not per-crate.\n"
             f"    Expected: {want_lint_inputs}."),
        ("a4-fmt", check_task_inputs(projects, crates, "fmt", FMT_TASK_INPUTS),
             "`fmt` does not key on everything `cargo fmt --check` actually reads, so a\n"
             "    rustfmt.toml edit, a toolchain bump or a misformatted tests/ file schedules\n"
             "    NOTHING for this crate (SMA-537).\n"
             "    Fix: the inputs are declared once for ALL crates in .moon/tasks/rust.yml —\n"
             "    restore them there, not per-crate. Expected: @group(sources), @group(tests),\n"
             "    /rs/rustfmt.toml, /rs/rust-toolchain.toml."),
        ("a5", a5,
             "An FFI build task does not key on the workspace-level files, so a dependency bump\n"
             "    replays a CACHED artifact built from a different resolution — and clippy cannot\n"
             "    cover it, because it never links a cdylib and never targets wasm32 (SMA-546).\n"
             f"    Fix: add {want_ffi_inputs}\n"
             "    to that task's `inputs`. A `not matched by any FFI marker` row\n"
             "    means the opposite — the task stopped looking like a Rust build to A5; either\n"
             "    restore the invocation or update FFI_MARKERS."),
        ("a6", check_upstream_inputs(projects),
             "A crate's build/test/lint does not key on its upstream crates' sources, so an\n"
             "    upstream change SELECTS NOTHING for this crate and its cached PASS replays\n"
             "    against a different upstream (SMA-528).\n"
             "    Fix: the list lives in that crate's own moon.yml under `fileGroups.upstreams` —\n"
             "    two entries per upstream, `/<src_dir>/src/**/*` and `/<src_dir>/Cargo.toml`,\n"
             "    for its TRANSITIVE dependsOn closure. A `not in its closure` row is the\n"
             "    opposite: delete the entry, or add it to ALLOW_OVER_APPROXIMATION with a reason.\n"
             "    A `FLOOR:` row means the check itself cannot be trusted — the crate is missing\n"
             "    from the graph, it dropped out of A6's examined set (e.g. stopped reporting\n"
             "    `language: rust`), or its dependsOn closure derivation is broken — fix that\n"
             "    first, every other A6 row is meaningless until it passes."),
        ("a7", check_wrapper_upstream_inputs(projects, root),
             "A py/ts wrapper's FFI task does not key on an upstream Rust crate's sources, so a\n"
             "    change there SELECTS NOTHING for that wrapper and the ADR-0005 parity replay\n"
             "    silently stops running on it (SMA-560).\n"
             "    Fix: add the missing entry to that task's `inputs` in the wrapper's own\n"
             "    moon.yml — `/<src_dir>/src/**/*` and `/<src_dir>/Cargo.toml` for every crate in\n"
             "    its TRANSITIVE dependsOn closure, plus `/<src_dir>/build.rs` and any\n"
             "    `/<src_dir>/*.pyi` stub, each where one exists on disk.\n"
             "    Extra inputs beyond the closure are ALLOWED (this is containment, unlike A6).\n"
             "    A `FLOOR:` row means the check itself cannot be trusted — the wrapper is\n"
             "    missing, its closure derivation broke, or its task stopped matching an FFI\n"
             "    marker — fix that first, every other A7 row is meaningless until it passes."),
        ("a8", a8,
             "Cargo-resolving task without --locked (it REPAIRS a truncated lock mid-run,\n"
             "    so every later --locked gate reads a lock the PR never shipped — SMA-601).\n"
             "    Fix: add `--locked` to the task's command, or add an ALLOW_UNLOCKED_CARGO\n"
             "    entry with the measured reason it cannot take one.\n"
             "    A `through a wrapper` row can ONLY be cleared by an allowlist entry: the\n"
             "    wrapper's own cargo call takes no flag, so a `--locked` elsewhere in the same\n"
             "    script does not cover it.\n"
             "    An `A8 examines` row means the opposite — the derivation stopped matching a\n"
             "    task it must cover; fix that first, every other A8 row is meaningless until\n"
             "    it passes.\n"
             "    A `<script>:<line>` row is inside a gate's own run.sh: add `--locked` there, or an\n"
             "    ALLOW_UNLOCKED_CARGO_SCRIPT entry keyed by (script, exact segment text). The scan is\n"
             "    path-insensitive — check by hand whether the task's arguments actually reach that\n"
             "    line before waiving it (SMA-599 L1)."),
        ("a9", check_member_globs(root, crates),
             "A workspace crate is unreachable through Dependabot's `[workspace] members`\n"
             "    expansion, so its cargo update job resolves a SHORTER workspace than Cargo\n"
             "    does — it proposes a truncated rs/Cargo.lock and reds on any dependency that\n"
             "    needs a companion package unlocked with it (SMA-604).\n"
             "    Fix: give each `members` entry at most ONE wildcard level\n"
             "    (`crates/libs/*`, never `crates/*/*`), one entry per crate directory.\n"
             "    Dependabot lists a single directory level below the glob's literal prefix.\n"
             "    An `A9 examines` row means the opposite — cargo_crates() found no crates, so\n"
             "    the comparison covers nothing; fix that first."),
        ("a10", check_cargo_config_inputs(projects, root),
             "A task runs a COMPILING cargo command with cwd inside rs/ but does not key on\n"
             "    rs/.cargo/config.toml, so a rustflags edit replays its cached result\n"
             "    (SMA-594, SMA-599).\n"
             "    Fix: for a crate task the input is declared once for ALL crates in\n"
             "    .moon/tasks/rust.yml — restore it there, not per-crate. For a repo:* gate it\n"
             "    is declared in that task's own `inputs` in moon.yml.\n"
             "    `cargo fmt`, `cargo tree`, `cargo metadata`, `cargo deny` and `cargo machete`\n"
             "    are out of scope BY VERB (they never compile or link) — see\n"
             "    CONFIG_SENSITIVE_VERBS. A `FLOOR:` row means the check itself cannot be\n"
             "    trusted; fix that first, every other A10 row is meaningless until it passes."),
    ]

    return findings


def main():
    root = Path(__file__).resolve().parents[2]
    try:
        projects = moon_projects()
        crates = cargo_crates(root)
        findings = collect_findings(projects, crates, root)
    except INFRA_ERRORS as exc:
        # Mirror run.sh's infra-vs-assertion split: a broken `moon` — or an unparseable Cargo.toml —
        # must never be mistaken for a graph regression. See INFRA_ERRORS.
        print(f"FATAL [parity] could not build the graphs: {exc}", file=sys.stderr)
        return 2

    if not any(rows for _, rows, _ in findings):
        print(
            f"PASS  {'cargo-moon-parity':<18} -> "
            f"{len(crates)} crates: every Cargo dep has a Moon edge that schedules its build, "
            f"every lint and fmt keys on the files its command reads, every FFI build task does "
            f"too, every crate keys on its upstream sources, and every py/ts wrapper keys on the "
            f"Rust crates it builds, every cargo-resolving task passes --locked, and every "
            f"workspace crate is reachable through Dependabot's member expansion, and "
            f"every compiling cargo task inside rs/ keys on .cargo/config.toml"
        )
        return 0

    print("FAIL  [cargo-moon-parity] Cargo and Moon disagree", file=sys.stderr)
    for _, rows, title in findings:
        if rows:
            print(f"  {title}", file=sys.stderr)
            for row in rows:
                print(f"      {row}", file=sys.stderr)
    return 1

if __name__ == "__main__":
    sys.exit(self_test() if "--self-test" in sys.argv[1:] else main())
