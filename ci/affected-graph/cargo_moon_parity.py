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
# A11 (SMA-663) reads the same `members` list and reds on any glob character in it, because a
# glob can match napi's `.<crate>.napi-stage-<random>` staging directory.
#
# A12 (SMA-536) is about ts `tsc` tasks: A12a asserts that each one keys on its package.json
# dependency closure, and A12b that each one whose closure holds a `file:` binding runs the
# installed-typings preflight before `tsc`. It reads package.json files from disk.
#
# A13 (SMA-736) is about ts vitest tasks: each one must key on its config files, its package.json
# closure, the runtime files of each `file:` binding, the tsconfig.json `extends` chains and each
# alias target outside its own package. It reads the vitest configs and tsconfig files from disk,
# and it asks git which files it tracks.
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

# SMA-736: A13 reads git's tracked set through repo:input-liveness's own helper. Python puts this
# script's directory (ci/affected-graph/) first on sys.path, so the sibling module imports directly.
import task_inputs


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
    # SMA-736: task_inputs.tracked_files raises task_inputs' own MoonOutputError class.
    task_inputs.MoonOutputError,
)

# (consumer, upstream) -> why this hand-declared Moon edge has no Cargo backing.
# An allowlisted edge is a RECORDED DECISION, not a silent exemption: the reason string is required.
# EMPTY since SMA-635: the one entry, ("paigasus-gateway-rs", "paigasus-kernel-rs"), is now backed
# by a real Cargo dependency (the gateway's `domain::resolve_org`).
ALLOW_NO_CARGO_BACKING: dict[tuple[str, str], str] = {}

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
# `... && uv run --isolated --reinstall-package paigasus-py-bindings pytest tests`. It is kept so a future direct maturin
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

# SMA-765 (A14) - the floor for the uv-isolation check. A14 scans EVERY task, so its derived set
# cannot shrink to empty. What can vanish silently is the kernel task's own isolation: the
# `--isolated` flag or the `--reinstall-package paigasus-py-bindings` that keeps the binding
# fresh. Each task named here must exist and must run `uv run --isolated` with
# `--reinstall-package paigasus-py-bindings`, or A14 reds with a `FLOOR:` row.
REQUIRED_UV_ISOLATED_TASKS = ("paigasus-kernel-py:test",)

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

# SMA-658 (2026-09-20): `rs/Dockerfile` runs `cargo auditable build --release --locked …` — a
# cargo SUBCOMMAND WRAPPER that forwards to the named verb and preserves whatever flags the verb
# accepts, `--locked` included. `cargo auditable` is the only such wrapper this repo ships, so
# the allowlist below names it explicitly rather than accepting an arbitrary `\S+` between
# `cargo` and the verb — a widened form would let an UNKNOWN wrapper (a typo, or a tool nobody
# reviewed) also read as a resolved invocation, silently defeating the floor the general text
# scan above already ruled out for the opposite reason (prose false positives). An unrecognised
# wrapper word must still read as "no invocation" and fail A8's floor closed, not open.
CARGO_SUBCOMMAND_WRAPPERS = ("auditable",)

# NOT the whole story since SMA-605: this is the LITERAL arm only. `cargo_matches` merges it with
# two INDIRECT arms — a cargo-named variable in command position (CARGO_VAR_CMD_RE) and the
# `CARGO=` environment prefix (CARGO_ENV_PREFIX_RE) — and every consumer reads that merged list,
# not this regex. A10 has its own sensitive-verb variant, CARGO_VAR_CMD_SENSITIVE_RE.
CARGO_INVOCATION_RE = re.compile(
    r"\bcargo\s+(?:\+\S+\s+)?(?:(?:" + "|".join(CARGO_SUBCOMMAND_WRAPPERS) + r")\s+)?(?:"
    + "|".join(LOCK_RESOLVING_VERBS) + r")\b"
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
# ci/release-parity/ecosystems/release-plz.sh:156, and only once script_source_refs makes that
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
# (ci/release-parity/ecosystems/release-plz.sh:156). The name is EXACTLY `CARGO`: CARGO_HOME,
# CARGO_TERM_COLOR and CARGO_NET_OFFLINE configure cargo without redirecting it, and line 156
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

# SMA-767 follow-up — the crate-local `.cargo/config.toml` files. cargo reads every
# `.cargo/config.toml` from its cwd up to the file system root, so a cargo run with cwd in a crate
# directory reads the crate-local file AND `rs/.cargo/config.toml`. uv and maturin build
# `paigasus-py-bindings` with cwd in its crate directory, which is why that crate ships the
# macOS `-C strip=none` flag in its own file (the sdist cannot carry `rs/.cargo/config.toml`).
# A10 demands each such file from every compiling task with cwd at or under its directory; A5
# demands it from every FFI task whose invocation names the crate. The floor: each path here
# must be demanded by A10 AND by A5 at least once, or the derivation has degraded.
REQUIRED_CRATE_CARGO_CONFIGS = ("rs/crates/bindings/paigasus-py-bindings/.cargo/config.toml",)


def crate_local_cargo_configs(root):
    """{crate directory name: workspace-relative path} of every `rs/crates/*/*/.cargo/config.toml`."""
    out = {}
    for path in sorted((Path(root) / "rs" / "crates").glob("*/*/.cargo/config.toml")):
        out[path.parent.parent.name] = str(path.relative_to(root))
    return out


def _cargo_configs_below_rs(root, source_dir):
    """The `.cargo/config.toml` paths that cargo reads between `source_dir` and `rs/`, `rs/` excluded."""
    out, d = [], Path(source_dir)
    while str(d).startswith("rs/"):
        rel = d / ".cargo" / "config.toml"
        if (Path(root) / rel).is_file():
            out.append(str(rel))
        d = d.parent
    return out


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
    "paigasus-kernel-ts:generate-wasm": (
        "as paigasus-kernel-ts:build — it reaches cargo through `wasm-pack build ... -- --locked`, "
        "and wasm-pack makes its OWN unlocked cargo call before that build and repairs the lock "
        "there (measured, SMA-601). SMA-634 made this task the only writer of the committed wasm "
        "artifacts; it is runInCI: false, so CI never runs it, but A8 reads the declaration."
    ),
    "paigasus-kernel-ts:generate-napi-glue": (
        "as paigasus-kernel-ts:build — it reaches cargo through `napi build`, which exposes no "
        "--locked and no cargo passthrough (SMA-601). SMA-667 made this task the sanctioned local "
        "writer of the committed napi glue; it is runInCI: false, so CI never runs it, but A8 "
        "reads the declaration."
    ),
    "paigasus-kernel-py:test": (
        "reaches cargo through `uv run --reinstall-package`, which drives maturin, which drives "
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
        "PURPOSE is to regenerate the lock after writing the ten version sites it owns "
        "(SMA-685). The "
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

# SMA-765, SMA-768 (A14) - waivers for a red uv line inside a script that a task runs. Keyed by
# (repo-relative script path, EXACT stripped PHYSICAL line text), not by line number, like
# ALLOW_UNLOCKED_CARGO_SCRIPT. The value is the non-empty reason. It starts with its class: PROSE (a
# message), DATA (a pattern or Python data) or EXECUTED (a real call). A14 does not strip strings,
# so a line that only NAMES `uv` is red until it is listed here. Exactly one entry is EXECUTED:
# the `uv lock` line in ci/version-lockstep/run.sh. Add no second EXECUTED entry without a review.
# An entry that matches no line of a scanned script is itself a row, so the list cannot rot.
ALLOW_UV_ENV_SCRIPT = {
    ("ci/ruff/run.sh",
     "|| die_infra \"could not resolve ruff via 'uv run --locked --project py' "
     "\u2014 run 'uv sync --project py'\""): (
        "PROSE, not an invocation: the diagnostic that tells the reader how to repair a missing "
        "ruff. The script executes no `uv sync`."
    ),
    ("ci/actionlint/run.sh",
     "|| infra \"could not resolve shellcheck via 'uv run --locked --project py' "
     "\u2014 run 'uv sync --project py'\""): (
        "PROSE, not an invocation: the diagnostic that tells the reader how to repair a missing "
        "shellcheck. The script executes no `uv sync`."
    ),
    (
        "ci/release-parity/ecosystems/python-semantic-release.sh",
        "\"Run 'uv sync' in py/, and check py/uv.lock still carries python-semantic-release.\" \\",
    ): (
        "PROSE, not an invocation: an argument of `_psr_fatal` that prints a remedy in an error "
        "message. The script executes no `uv sync`."
    ),
    ('ci/actionlint/run.sh',
     'echo "                 shells out to \'uv run --locked --project py\', so it needs uv on PATH and" >&2'): (
        'PROSE, not an invocation: a usage message that names the `uv run` call the script '
        'makes elsewhere. The script executes no `uv` here.'
    ),
    ('ci/actionlint/run.sh',
     'echo "                 shells out to \'uv run --project ci/release-plan\', which needs uv on" >&2'): (
        'PROSE, not an invocation: a usage message that names the `uv run` call the script '
        'makes elsewhere. The script executes no `uv` here.'
    ),
    ('ci/actionlint/run.sh',
     '[ "$pc_rc" -eq 0 ] || infra "the pipe-capacity probe failed (rc $pc_rc) via \'uv run --locked --project py\'. No check ran."'): (
        'PROSE, not an invocation: the `infra` message when the pipe-capacity probe fails. '
        'The script executes no `uv` here.'
    ),
    ('ci/actionlint/run.sh',
     "rc 0 having asserted nothing — MEASURED at rc 127. A missing 'uv' produces 127 from the"): (
        'PROSE, not an invocation: a line inside a multi-line comment text that names a '
        'missing `uv`. The script executes no `uv` here.'
    ),
    ('ci/actionlint/run.sh',
     'infra "check 11: ci/release-plan/run.sh --assert aborted (exit 2) — uv or the interpreter'): (
        'PROSE, not an invocation: an `infra` message that says `uv` or the interpreter '
        'failed. The script executes no `uv` here.'
    ),
    ('ci/helm-render/run.sh',
     'command -v uv >/dev/null 2>&1 || die_infra "uv is not on PATH; run \'proto install\', or add ~/.proto/shims to PATH"'): (
        'PROSE, not an invocation: the `die_infra` message after the `command -v uv` probe. '
        'The probe is an approved form. The script executes no `uv` here.'
    ),
    ('ci/helm-render/run.sh',
     '|| die_infra "uv could not provide the locked PyYAML environment for $HERE"'): (
        'PROSE, not an invocation: a `die_infra` message that says `uv` could not provide the '
        'environment. The script executes no `uv` here.'
    ),
    ('ci/osv/run.sh',
     'echo "  npm transitive, or \'uv lock --upgrade-package <name>\' for pip). Waive ONLY with a" >&2'): (
        'PROSE, not an invocation: a usage hint that tells the reader how to relock. The '
        'script executes no `uv lock`. The script executes no `uv` here.'
    ),
    ('ci/version-lockstep/run.sh',
     'lpos="$(grep -Fn \'( cd "$REPO_ROOT/py" && uv lock\' < <(printf \'%s\\n\' "$wbody"))" || lpos=""'): (
        'DATA, not an invocation: a `grep -F` pattern in a self-test that looks for the `uv '
        'lock` text in the `run_write` body. It is data, not a call.'
    ),
    ('ci/version-lockstep/run.sh',
     '|| { fail "self-test: run_write must run uv_static_metadata_check before uv lock"; return 1; }'): (
        'PROSE, not an invocation: a `fail` message of a self-test. The script executes no '
        '`uv` here.'
    ),
    ('ci/version-lockstep/run.sh',
     '|| { fail "self-test: run_write no longer maps the uv metadata check\'s rc 1 to exit 1"; return 1; }'): (
        'PROSE, not an invocation: a `fail` message of a self-test. The script executes no '
        '`uv` here.'
    ),
    ('ci/version-lockstep/run.sh',
     "0) printf 'uv workspace: every local package declares static metadata\\n' ;;"): (
        'PROSE, not an invocation: a `printf` status message. The script executes no `uv` '
        'here.'
    ),
    ('ci/version-lockstep/run.sh',
     '|| { rm -rf "$dest"; die_infra "cannot list the uv workspace pyproject files to stage"; }'): (
        'PROSE, not an invocation: a `die_infra` message. The script executes no `uv` here.'
    ),
    ('ci/version-lockstep/run.sh',
     'uvws = ws.get("tool", {}).get("uv", {}).get("workspace")'): (
        'DATA, not an invocation: Python data in a heredoc: `ws.get("tool", {}).get("uv", '
        '{})` reads the `[tool.uv]` table. It is data, not a call.'
    ),
    ('ci/version-lockstep/run.sh',
     'print(f"INFRA: the uv workspace member glob {pat!r} matches no directory", file=sys.stderr)'): (
        'PROSE, not an invocation: a Python `INFRA:` message in a heredoc. The script '
        'executes no `uv` here.'
    ),
    ('ci/version-lockstep/run.sh',
     'violations.append(f"{rel(p)}: no [project] table, so uv would run the build backend to read its metadata")'): (
        'PROSE, not an invocation: a Python violation message in a heredoc. The script '
        'executes no `uv` here.'
    ),
    ('ci/version-lockstep/run.sh',
     'violations.append(f"{rel(p)}: [project].dynamic lists \'{field}\', so uv lock would run the build backend")'): (
        'PROSE, not an invocation: a Python violation message in a heredoc. The script '
        'executes no `uv` here.'
    ),
    ('ci/version-lockstep/run.sh',
     'sources = doc.get("tool", {}).get("uv", {}).get("sources", {})'): (
        'DATA, not an invocation: Python data in a heredoc: `doc.get("tool", {}).get("uv", '
        '{})` reads the `[tool.uv]` table. It is data, not a call.'
    ),
    ('ci/version-lockstep/run.sh',
     'violations.append(f"{rel(p)}: [tool.uv.sources] {name} path {entry[\'path\']!r} is not a directory, so uv would build it")'): (
        'PROSE, not an invocation: a Python violation message in a heredoc. The script '
        'executes no `uv` here.'
    ),
    ('ci/wasm-lockstep/run.sh',
     'command -v uv >/dev/null 2>&1 || die_infra "uv is not on PATH; run \'proto install\', or add ~/.proto/shims to PATH"'): (
        'PROSE, not an invocation: the `die_infra` message after the `command -v uv` probe. '
        'The probe is an approved form. The script executes no `uv` here.'
    ),
    ('ci/wasm-lockstep/run.sh',
     '|| die_infra "uv could not provide the locked PyYAML environment for $HERE"'): (
        'PROSE, not an invocation: a `die_infra` message that says `uv` could not provide the '
        'environment. The script executes no `uv` here.'
    ),
    ('ci/workflow-credentials/run.sh',
     '|| die_infra "uv is not on PATH — run \'proto install\', or add ~/.proto/shims to PATH"'): (
        'PROSE, not an invocation: a `die_infra` message that says `uv` is not on PATH. The '
        'script executes no `uv` here.'
    ),
    ('ci/workflow-credentials/run.sh',
     '*) die_infra "checker exited $rc — uv or the interpreter failed, not an assertion" ;;'): (
        'PROSE, not an invocation: a `die_infra` message that says `uv` or the interpreter '
        'failed. The script executes no `uv` here.'
    ),
    ('ci/release-plan/run.sh',
     '|| die_infra "uv is not on PATH — run \'proto install\', or add ~/.proto/shims to PATH"'): (
        'PROSE, not an invocation: a `die_infra` message that says `uv` is not on PATH. The script executes no `uv` here. Scanned since follow-up sweep 2 (R8): ci/actionlint/run.sh runs this script.'
    ),
    ('ci/release-plan/run.sh',
     '*) die_infra "checker exited $rc — uv or the interpreter failed, not an assertion" ;;'): (
        'PROSE, not an invocation: a `die_infra` message that says `uv` or the interpreter failed. The script executes no `uv` here.'
    ),
    ('ci/release-plan/run.sh',
     "printf '  FAIL row 6 proves nothing: uv is still reachable under the restricted PATH\\n' >&2"): (
        'PROSE, not an invocation: a self-test `printf` message. The script executes no `uv` here.'
    ),
    ('ci/release-plan/run.sh',
     "printf '  FAIL --github-output exited %s with uv unreachable, expected 0 (the fail-safe arm)\\n' \\"): (
        'PROSE, not an invocation: a self-test `printf` message. The script executes no `uv` here.'
    ),
    ('ci/release-plan/run.sh',
     "printf '  FAIL --github-output with uv unreachable did not write nothing_to_release=false\\n' >&2"): (
        'PROSE, not an invocation: a self-test `printf` message. The script executes no `uv` here.'
    ),
    ('ci/release-plan/run.sh',
     '_expect_failsafe_outputs "row 6 (uv unreachable)" "$nouv_out"'): (
        'PROSE, not an invocation: a self-test row label. The script executes no `uv` here.'
    ),
    ('ci/actionlint/run.sh',
     'done < <(grep -nE \'^[[:blank:]]*(command|env|time|eval|exec|if|while|until|!)([[:blank:]]+(command|env|time|eval|exec|if|while|until|!|[A-Za-z_][A-Za-z0-9_]*=[^[:blank:]]*))*[[:blank:]]+moon[[:blank:]]+(ci|run)([[:blank:]]|$)\' "$f")'): (
        'DATA, not an invocation: a `grep -E` pattern that lists `eval` as a command wrapper word to find. It is data, not a call (R2 `eval` rule).'
    ),
    ('ci/actionlint/run.sh',
     'command wrapper (command/env/time/eval/exec/if/while/until/!) on the same physical line, so'): (
        'PROSE, not an invocation: a `fail` message that names `eval` as a wrapper word. The script runs no `eval` here (R2 `eval` rule).'
    ),
    ('ci/version-lockstep/run.sh',
     '( cd "$REPO_ROOT/py" && uv lock >/dev/null ) || die_infra "uv lock failed (site 17)"'): (
        'EXECUTED: this `uv lock` writes py/uv.lock. The line is in `run_write`, and no Moon '
        'task passes `--write` to the script, so no Moon task runs it. The A14 `--write` '
        'guard keeps this true.'
    ),
}

# A `scripts/*.sh` reference inside a project, resolved against the project's `source_dir`
# (`py:test` runs `bash scripts/run_tests.sh`). SCRIPT_REF_RE covers only the `ci/` form.
# A leading `./` or `../` is part of the reference. The path is resolved with those segments, so
# `../scripts/x.sh` is read relative to the project dir. A ref that does not resolve to a file
# raises MoonOutputError (fail-closed), and so does a ref that leaves the repository root.
PROJECT_SCRIPT_REF_RE = re.compile(
    r"(?:^|[\s;&|(])(?:bash\s+|sh\s+)?((?:\.{1,2}/)*scripts/[\w./-]+\.sh)\b"
)


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
    for lineno, raw in enumerate(_read_utf8(path).splitlines(), 1):
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
        "ci/release-parity/ecosystems/release-plz-filter.sh",
        "ci/release-parity/ecosystems/release-plz.sh",
        "ci/release-parity/ecosystems/semantic-release.sh",
    ),
    # SMA-716: the release-plz module sources the release-plz-only suites.
    "ci/release-parity/ecosystems/release-plz.sh": (
        "ci/release-parity/ecosystems/release-plz-filter.sh",
    ),
}


def _read_utf8(path):
    """`path`'s text, read as UTF-8. A file that is not UTF-8 raises MoonOutputError that names it.

    Every file read in this module goes through this helper (SMA-768 follow-up). A bare
    `read_text()` raised a raw `UnicodeDecodeError`: rc 1 and a traceback with no file name.
    MoonOutputError is in INFRA_ERRORS, so main() exits 2. A13's readers catch it and give a row
    that names the file, which is A13's contract for an unreadable file. The self-test pins that
    this is the only `read_text` call in the module.
    """
    try:
        return Path(path).read_text(encoding="utf-8")
    except UnicodeDecodeError as exc:
        raise MoonOutputError(
            f"{path}: not valid UTF-8 ({exc.reason} at byte {exc.start}) - the scan cannot read it"
        ) from exc


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
    for raw in _read_utf8(path).splitlines():
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


def script_source_refs(path, root, stmt_re=None):
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
    for raw in (stmt_re or SOURCE_STMT_RE).findall(text):
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


def check_cargo_config_inputs(projects, root, allow=None, floor=None, local_floor=None):
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
    # `local_floor` defaults to the live floor only on the live call (no `floor` given), so the
    # synthetic fixtures, which pass their own `floor`, do not need a crate-local file.
    if local_floor is None:
        local_floor = REQUIRED_CRATE_CARGO_CONFIGS if floor is None else ()
    allow = ALLOW_MISSING_CARGO_CONFIG if allow is None else allow
    floor = REQUIRED_CARGO_CONFIG_TASKS if floor is None else floor
    rows, in_scope, demanded = [], set(), set()
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
            text += "\n" + _read_utf8(path)
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
        # SMA-767 follow-up: a crate-local `.cargo/config.toml` at or above the task's cwd.
        for local in _cargo_configs_below_rs(root, projects[pid]["source_dir"]):
            demanded.add(local)
            if local not in set(files) | set(globs):
                rows.append(
                    f"{target} runs a compiling cargo command in {projects[pid]['source_dir']} but "
                    f"does not key on {local}, which cargo reads from that cwd — a rustflags edit "
                    f"replays its cached result"
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
    for local in sorted(set(local_floor) - demanded):
        rows.append(
            f"FLOOR: A10 demands {local} from no task — the file moved, or the crate-local "
            f"derivation has degraded (see REQUIRED_CRATE_CARGO_CONFIGS)"
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
    for lineno, line in enumerate(_read_utf8(path).splitlines(), 1):
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


def read_workspace_members(root, assertion):
    """Return rs/Cargo.toml's `[workspace] members` list, for A9 and A11.

    An absent manifest or an absent/empty `members` key is infrastructure, never a silent pass.
    Both would make the assertion vacuous while it still prints PASS.
    """
    path = root / "rs" / "Cargo.toml"
    if not path.is_file():
        raise MoonOutputError(
            f"{path} is absent — {assertion}'s members assertion cannot be evaluated. If the "
            f"workspace root legitimately moved, update read_workspace_members rather than "
            f"deleting the check"
        )
    members = (tomllib.loads(_read_utf8(path)).get("workspace") or {}).get("members")
    if not members:
        raise MoonOutputError(
            f"{path} declares no `[workspace] members` — {assertion} cannot examine a member "
            f"set that does not exist"
        )
    return members


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
    members = read_workspace_members(root, "A9")

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


MEMBER_GLOB_CHARS = ("*", "?", "[")


def check_member_literals(root):
    """Return the A11 violation list: `[workspace] members` entries that carry a glob (SMA-663).

    A glob can match a dot-directory. `napi build` (@napi-rs/cli 3.10.3, dist/cli.js:10350)
    stages every output in `mkdtemp(dirname(outputDir) + "/." + basename(outputDir) +
    ".napi-stage-")`, a sibling of the crate with no Cargo.toml in it. While it exists, every
    concurrent `cargo metadata` fails with exit 101 on the missing manifest. `moon ci` runs the
    two napi tasks in parallel with the crate tasks, so the race reached 9 of 9 runs on SMA-658.

    The rule is deliberately blunt. A finer rule ("no glob may match a napi staging path")
    would have to model how napi resolves its crate. It reads `--cwd`, `--manifest-path`, `-p`
    and `-o` to do that, and a wrong model passes in silence. A literal list cannot drift: A9
    reds on a crate directory that no entry reaches.
    """
    return [
        f"members entry {entry!r} carries a glob character. "
        f"It can match a dot-directory such as napi's `.<crate>.napi-stage-<random>`, "
        f"and a concurrent `cargo metadata` then fails on its missing Cargo.toml"
        for entry in read_workspace_members(root, "A11")
        if any(ch in entry for ch in MEMBER_GLOB_CHARS)
    ]


def check_ffi_inputs(projects, required=FFI_TASK_INPUTS, floor=REQUIRED_FFI_TASKS, root=None, local_floor=None):
    """Return the A5 violation list: FFI-compiling tasks that do not key on the workspace files.

    Two halves, and both are load-bearing:

    * DERIVED — any task whose resolved invocation matches an FFI marker must declare `required`.
      This is what covers a future fourth binding task on the day it is added.
    * FLOOR — every task in `floor` must appear in the derived set. Without this a derivation that
      silently stops matching (a renamed flag, an invocation moved behind a wrapper script, a moon
      upgrade dropping `script`) degrades to an empty set and a vacuous PASS.

    The derivation itself lives in `derive_ffi_tasks`, shared with A7.

    With `root` (the live call), a derived task whose invocation names a crate that has a
    crate-local `.cargo/config.toml` must declare that file too (SMA-767 follow-up), and each
    `local_floor` path (default REQUIRED_CRATE_CARGO_CONFIGS) must be demanded at least once.

    Raises MoonOutputError if a task exposes none of a command, a script, or any args.
    """
    matched, a5 = derive_ffi_tasks(projects), []
    configs = crate_local_cargo_configs(root) if root is not None else {}
    if local_floor is None:
        local_floor = REQUIRED_CRATE_CARGO_CONFIGS if root is not None else ()
    demanded = set()
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
        blob = (projects[pid].get("invocations") or {}).get(name) or ""
        for crate, local in sorted(configs.items()):
            if re.search(rf"(?<![\w-]){re.escape(crate)}(?![\w-])", blob):
                demanded.add(local)
                if local not in resolved:
                    missing.append(local)
        if missing:
            a5.append(f"{target} inputs omit {', '.join(missing)}")
    for local in sorted(set(local_floor) - demanded):
        a5.append(
            f"FLOOR: A5 demands {local} from no FFI task — the file moved, or no derived task "
            f"names its crate any more (see REQUIRED_CRATE_CARGO_CONFIGS)"
        )
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


# SMA-536 — A12. Every ts task that runs `tsc` must key on what `tsc` reads through its package.json
# dependency closure (A12a), and must run the installed-typings preflight before `tsc` when that
# closure holds a `file:` binding (A12b). See
# docs/superpowers/specs/2026-10-04-sma-536-ts-typecheck-upstream-inputs-design.md.
#
# `tsc` as a bounded token, optionally behind `pnpm exec`. Bounded, so `tsc-alias`, `tscheck` and
# the `.d.ts` in `--dts index.fresh.d.ts` never match. A `tsc` behind a wrapper script is invisible
# (spec R3); the floor catches the loss of a known task, not a new wrapper.
TSC_TOKEN_RE = re.compile(r"(^|[\s;&|(])(pnpm\s+exec\s+)?tsc(\s|$)")
PREFLIGHT_SCRIPT = "ts/scripts/check-installed-bindings.mjs"
# `node <relative prefix>ts/scripts/check-installed-bindings.mjs` as a whole word. moon reports a
# script-form task with `command` set to the script's first word, so the joined blob reads
# `node node ../../../ts/...`. The leading-separator group finds the second `node`.
PREFLIGHT_RE = re.compile(r"(?:^|[\s;&|(])node\s+(?:\S*/)?ts/scripts/check-installed-bindings\.mjs(?=$|[\s;&|)])")
TS_BASE_TSCONFIG = "ts/tsconfig.base.json"
PROTO_PACKAGE = "@paigasus/proto"
# A DIRECT task dep, because contracts:generate deletes ts/packages/paigasus-proto/src/generated
# before it rewrites it. Direct matches repo policy (iam-console/moon.yml) and what moon_projects()
# records for each task.
PROTO_GENERATE_DEP = "contracts:generate"
TS_DEP_FIELDS = ("dependencies", "devDependencies", "peerDependencies")
TS_PACKAGE_GLOBS = ("ts/packages/*/package.json", "ts/apps/*/package.json")

# A12's anti-vacuity floors. A12 asserts CONTAINMENT, and a containment check whose `want` set
# empties passes having asserted nothing (A7's lesson). The task floor catches a derivation that
# stops matching a known task. The closure floor catches a package.json walk that stops deriving.
REQUIRED_TSC_TASKS = (
    "paigasus-kernel-ts:build",
    "paigasus-kernel-ts:typecheck",
    "paigasus-console-core-ts:typecheck",
    "iam-console-ts:typecheck",
    "gateway-console-ts:typecheck",
)
REQUIRED_TS_CLOSURE = {
    "@paigasus/kernel": {"@paigasus/node-bindings", "@paigasus/wasm"},
    "@paigasus/console-core": {"@paigasus/kernel"},
}


def _derive_ts_tasks(projects, token_re, label):
    """Every `<pid>:<task>` of a `language: typescript` project whose resolved invocation matches `token_re`.

    Raises MoonOutputError if a task exposes none of a command, a script, or any args. `label`
    names the derivation in the message.
    """
    matched = set()
    for pid in sorted(projects):
        proj = projects[pid]
        if proj.get("language") != "typescript":
            continue
        invocations = proj.get("invocations") or {}
        for name in sorted(invocations):
            blob = invocations[name]
            if blob is None:
                raise MoonOutputError(
                    f"{pid}:{name} reported none of a `command`, a `script`, or any `args` — "
                    f"moon's output shape changed, so the {label} derivation cannot be evaluated"
                )
            if token_re.search(blob):
                matched.add(f"{pid}:{name}")
    return matched


def derive_tsc_tasks(projects):
    """Every `<pid>:<task>` of a `language: typescript` project whose resolved invocation runs `tsc`.

    Shared by A12a and A12b. Raises MoonOutputError if a task exposes none of a command, a script,
    or any args, exactly as derive_ffi_tasks does.
    """
    return _derive_ts_tasks(projects, TSC_TOKEN_RE, "tsc")


def _read_package_json(root, rel_dir, rows):
    """Parse `<root>/<rel_dir>/package.json`. On any failure, append a row and return None.

    A row, never an infrastructure error and never a skip (spec §3.3): a broken manifest is an
    authoring mistake in this repo, and the row names the file.
    """
    rel = f"{rel_dir}/package.json"
    try:
        data = json.loads(_read_utf8(root / rel))
    except FileNotFoundError:
        rows.append(f"{rel} is missing")
        return None
    except (OSError, MoonOutputError) as exc:
        rows.append(f"{rel} cannot be read ({exc})")
        return None
    except ValueError as exc:
        rows.append(f"{rel} is not valid JSON ({exc})")
        return None
    if not isinstance(data, dict):
        rows.append(f"{rel} is not a JSON object")
        return None
    return data


def ts_workspace_packages(root, rows):
    """{package name: workspace-relative dir} for every ts/packages/* and ts/apps/* package."""
    names = {}
    for pattern in TS_PACKAGE_GLOBS:
        for path in sorted(root.glob(pattern)):
            rel_dir = path.parent.relative_to(root).as_posix()
            data = _read_package_json(root, rel_dir, rows)
            if data is None:
                continue
            name = data.get("name")
            if not isinstance(name, str) or not name:
                rows.append(f"{rel_dir}/package.json has no `name`, so no `workspace:` specifier can reach it")
                continue
            if name in names:
                rows.append(f"{name} is declared by both {names[name]} and {rel_dir}")
                continue
            names[name] = rel_dir
    return names


def ts_closure(root, own_dir, names, rows):
    """Transitive `workspace:` and `file:` closure of the package at `own_dir`.

    Returns {name: (kind, dir)} with kind "workspace" or "file". Walks `dependencies`,
    `devDependencies` and `peerDependencies` of every member, upstream devDependencies included:
    the apps reach @paigasus/proto through console-core's devDependency, because `tsc` reads
    console-core/testing/fake-iam.ts. The own package is excluded at every depth, and a visited
    set ends a cycle. Each fail-closed case appends a row to `rows`.
    """
    closure = {}
    seen = {own_dir}
    stack = [own_dir]
    while stack:
        cur = stack.pop()
        data = _read_package_json(root, cur, rows)
        if data is None:
            continue
        for field in TS_DEP_FIELDS:
            deps = data.get(field)
            if deps is None:
                continue
            if not isinstance(deps, dict):
                rows.append(f"{cur}/package.json `{field}` is not an object")
                continue
            for name in sorted(deps):
                spec = deps[name]
                if not isinstance(spec, str):
                    rows.append(f"{cur}/package.json: {name} has a specifier that is not a string")
                    continue
                if spec.startswith("link:"):
                    rows.append(
                        f"{cur}/package.json: {name} uses a `link:` specifier ({spec}); A12 has no "
                        f"rule for it, so handle it on purpose (SMA-536)"
                    )
                    continue
                if spec.startswith("workspace:"):
                    target = names.get(name)
                    if target is None:
                        rows.append(
                            f"{cur}/package.json: the `workspace:` dependency {name} maps to no package "
                            f"directory under ts/packages or ts/apps"
                        )
                        continue
                    kind = "workspace"
                elif spec.startswith("file:"):
                    target = os.path.normpath(os.path.join(cur, spec[len("file:"):]))
                    if not (root / target).is_dir():
                        rows.append(f"{cur}/package.json: {name} points at `{spec}`, which does not exist")
                        continue
                    kind = "file"
                else:
                    continue
                if target in seen:
                    continue
                seen.add(target)
                closure[name] = (kind, target)
                stack.append(target)
    return closure


def _export_leaves(node, where, rows):
    """The string leaves of an `exports` value. A condition object is walked; a null is ignored."""
    if node is None:
        return []
    if isinstance(node, str):
        return [node]
    if isinstance(node, dict):
        leaves = []
        for key in sorted(node):
            leaves += _export_leaves(node[key], where, rows)
        return leaves
    rows.append(
        f"{where}: `exports` holds a {type(node).__name__}, which A12 cannot read (it walks strings, "
        f"condition objects and null only)"
    )
    return []


def workspace_package_inputs(pkg_dir, data, rows):
    """The inputs one workspace package `pkg_dir` adds to a task that reads it through its closure.

    `pkg_dir/package.json`, `pkg_dir/src/**/*`, and each `exports` target outside `src/`:
    `pkg_dir/<head>/**/*` for a target in a subdirectory, `pkg_dir/<file>` for a file at the package
    root. `data` is the parsed package.json. A12a (`tsc`) and A13 (vitest) both call this, so the two
    rules cannot drift (SMA-736 spec §4.3 item 3). An `exports` shape it cannot read is a row.
    """
    want = {f"{pkg_dir}/package.json", f"{pkg_dir}/src/**/*"}
    for leaf in _export_leaves(data.get("exports"), f"{pkg_dir}/package.json", rows):
        if not leaf.startswith("./"):
            rows.append(f"{pkg_dir}/package.json: the `exports` target {leaf!r} does not start with `./`")
            continue
        rel = leaf[2:]
        if rel.startswith("src/"):
            continue
        head, sep, _ = rel.partition("/")
        want.add(f"{pkg_dir}/{head}/**/*" if sep else f"{pkg_dir}/{rel}")
    return want


def tsc_required_inputs(root, closure, rows):
    """Return (want, has_binding): the inputs one `tsc` task must declare for its closure (spec §3.3)."""
    want = {TS_BASE_TSCONFIG}
    has_binding = False
    for name in sorted(closure):
        kind, pkg_dir = closure[name]
        if kind == "file":
            has_binding = True
        want.add(f"{pkg_dir}/package.json")
        data = _read_package_json(root, pkg_dir, rows)
        if data is None:
            continue
        if kind == "workspace":
            want |= workspace_package_inputs(pkg_dir, data, rows)
            continue
        files = data.get("files") or []
        if not isinstance(files, list):
            rows.append(f"{pkg_dir}/package.json `files` is not a list")
            continue
        for entry in files:
            if isinstance(entry, str) and entry.endswith(".d.ts"):
                want.add(f"{pkg_dir}/{entry}")
    if has_binding:
        want.add(PREFLIGHT_SCRIPT)
    return want, has_binding


def ts_tsc_analysis(projects, root):
    """The derivation A12a and A12b share.

    Returns (tasks, per_task, names, walk_rows). `per_task[target]` holds `own_name` (the task's
    own package name or None), `closure`, `want` and `binding` (the closure holds a `file:`
    binding). `walk_rows` holds every fail-closed row from reading package.json files.
    """
    walk_rows = []
    tasks = derive_tsc_tasks(projects)
    names = ts_workspace_packages(root, walk_rows)
    per_task = {}
    for target in sorted(tasks):
        pid, _, _task = target.partition(":")
        own = projects[pid]["source_dir"]
        own_pkg = _read_package_json(root, own, walk_rows)
        closure = ts_closure(root, own, names, walk_rows)
        want, binding = tsc_required_inputs(root, closure, walk_rows)
        per_task[target] = {
            "own_name": own_pkg.get("name") if own_pkg else None,
            "closure": closure,
            "want": want,
            "binding": binding,
        }
    return tasks, per_task, names, list(dict.fromkeys(walk_rows))


def check_ts_tsc_inputs(projects, root, floor=REQUIRED_TSC_TASKS, closure_floor=REQUIRED_TS_CLOSURE):
    """Return the A12a violation list: `tsc` tasks that do not key on their package.json closure.

    CONTAINMENT, per task, over both input buckets — the shape of A7. The lists are hand-written
    and correctly hold other entries, so strict equality would be wrong. A task that reports no
    input bucket is a violation, never a skip.

    `root` is POSITIONAL AND REQUIRED, never defaulted, for the reason in A7's docstring (SMA-560
    I3): the package.json walk is the whole assertion, and a `root=None` default would make it
    opt-in.
    """
    tasks, per_task, names, walk_rows = ts_tsc_analysis(projects, root)
    rows = []
    for target in sorted(set(floor or ()) - tasks):
        rows.append(
            f"FLOOR: {target} is not matched by the `tsc` token, so A12 asserts nothing about it "
            f"(see TSC_TOKEN_RE; a `tsc` behind a wrapper script is invisible)"
        )
    for pkg, required in sorted((closure_floor or {}).items()):
        pkg_dir = names.get(pkg)
        if pkg_dir is None:
            rows.append(
                f"FLOOR: {pkg} is not a package under ts/packages or ts/apps, so its closure cannot "
                f"be derived"
            )
            continue
        derived = ts_closure(root, pkg_dir, names, walk_rows)
        for missing in sorted(set(required) - set(derived)):
            rows.append(
                f"FLOOR: the closure of {pkg} no longer derives {missing} — the package.json walk "
                f"is broken, so A12 asserts nothing"
            )
    rows += walk_rows
    for target in sorted(tasks):
        pid, _, task = target.partition(":")
        proj = projects[pid]
        info = per_task[target]
        files = (proj.get("task_inputs") or {}).get(task)
        globs = (proj.get("task_input_globs") or {}).get(task)
        if files is None or globs is None:
            rows.append(
                f"{target} reported no `inputFiles`/`inputGlobs` — moon's output shape changed, so "
                f"this assertion cannot be evaluated (treated as a violation, never skipped)"
            )
        else:
            observed = set(files) | set(globs)
            for entry in sorted(info["want"] - observed):
                rows.append(f"{target} inputs omit {entry}")
        reads_proto = info["own_name"] == PROTO_PACKAGE or PROTO_PACKAGE in info["closure"]
        deps = (proj.get("tasks") or {}).get(task) or []
        if reads_proto and PROTO_GENERATE_DEP not in deps:
            rows.append(
                f"{target} deps omit {PROTO_GENERATE_DEP} — it reads @paigasus/proto's generated "
                f"tree, so it must run after the generator for a deterministic cache key"
            )
    return list(dict.fromkeys(rows))


# The joins between the preflight and `tsc` that let `tsc` run after a failed preflight. `||` is
# checked before `|`, because it contains it. `&&` is removed first, so a lone `&` is a background job.
_PREFLIGHT_BAD_JOINS = (("||", "`||`"), (";", "`;`"), ("\n", "a newline"), ("|", "`|`"), ("&", "`&`"))


def preflight_order_verdict(blob):
    """None if `blob` runs the preflight before its first `tsc`, joined only with `&&`; else why not."""
    tsc = TSC_TOKEN_RE.search(blob)
    if tsc is None:
        return "runs no `tsc` token"
    pre = PREFLIGHT_RE.search(blob)
    if pre is None:
        return f"does not run {PREFLIGHT_SCRIPT} before `tsc`, so `tsc` can read stale installed typings"
    if pre.start() > tsc.start():
        return f"runs {PREFLIGHT_SCRIPT} after `tsc`, so `tsc` can read stale installed typings first"
    # Up to the END of the separator group, so the character just before `tsc` (a newline, a `;`)
    # is part of what is checked.
    leftover = blob[pre.end():tsc.end(1)].replace("&&", "")
    for op, shown in _PREFLIGHT_BAD_JOINS:
        if op in leftover:
            return (
                f"joins {PREFLIGHT_SCRIPT} to `tsc` with {shown}, not `&&`, so a failed preflight "
                f"does not stop `tsc`"
            )
    return None


def check_ts_tsc_preflight(projects, root, floor=REQUIRED_TSC_TASKS):
    """Return the A12b violation list: binding `tsc` tasks that do not run the preflight first.

    Examines every task A12 derives whose closure holds a `file:` binding. The package.json walk
    rows are A12a's to report; A12b drops them, so one broken manifest does not print under two
    titles. `root` is positional and required, as in A12a.
    """
    tasks, per_task, _names, _walk_rows = ts_tsc_analysis(projects, root)
    examined = {target for target in tasks if per_task[target]["binding"]}
    rows = []
    for target in sorted(set(floor or ()) - examined):
        rows.append(
            f"FLOOR: {target} is in the A12 floor, but A12b does not examine it — it runs no `tsc` "
            f"token, or its package closure no longer holds a `file:` binding"
        )
    for target in sorted(examined):
        pid, _, task = target.partition(":")
        verdict = preflight_order_verdict(projects[pid]["invocations"][task])
        if verdict:
            rows.append(f"{target} {verdict}")
    return rows


# SMA-736 — A13. Every ts task that runs vitest must key on what vitest reads: its config files, the
# package.json closure that A12 walks, the runtime files of each `file:` binding, the tsconfig.json
# `extends` chains, and each alias target outside its own package. See
# docs/superpowers/specs/2026-10-09-sma-736-vitest-upstream-inputs-design.md.
#
# The config parser below is deliberately narrow (spec §4.4). It reads the alias forms the real
# configs use. Any other form that can carry an alias is a row, never a skip.
_JS_IDENT_RE = re.compile(r"[A-Za-z_$][\w$]*")
_JS_IMPORT_RE = re.compile(r"(?<![\w$.])(?:from|import|require)\s*\(?\s*(?=['\"])")
_JS_SPLIT_CONFIG_RE = re.compile(r"(?<![\w$])(mergeConfig|extends)(?![\w$])")
_JS_CONST_STRING_RE = re.compile(r"(?<![\w$])const\s+([A-Za-z_$][\w$]*)\s*=\s*(?=['\"])")
_JS_CONST_URL_RE = re.compile(r"(?<![\w$])const\s+([A-Za-z_$][\w$]*)\s*=\s*fileURLToPath\(\s*new\s+URL\(\s*(?=['\"])")
_JS_URL_TAIL_RE = re.compile(r"\s*,\s*import\.meta\.url\s*\)\s*\)")
_JS_STATEMENT_END_RE = re.compile(r"[ \t]*(?:;|\n|$)")
_JS_ALIAS_WORD_RE = re.compile(r"(?<![\w$])alias(?![\w$])")
_JS_KEY_COLON_RE = re.compile(r"\s*:")
_JS_COLON_RE = re.compile(r"\s*:\s*")
_JS_TSCONFIG_OFF_RE = re.compile(r"(?<![\w$])tsconfig\s*:\s*false(?![\w$])")
_JS_PROJECTS_RE = re.compile(r"(?<![\w$])projects\s*:\s*")
_JS_REGEX_PREV = "(,=:[!&|?{};+-*%<>~^"
# A keyword before a `/` means an expression starts there, so the `/` opens a regex literal.
_JS_REGEX_KEYWORD_RE = re.compile(r"(?<![\w$.])(?:return|typeof|in|of|case|void|delete|throw|new|yield|await|instanceof|else|do)\s*$")
_JS_OPEN = "([{"
_JS_CLOSE = ")]}"


def _scan_js(text, where, rows):
    """One pass over a JS, TS or JSONC text. Return (code, masked, literals), or None after a row.

    The scanner has one state at a time: code, `'...'`, `"..."`, a template literal, a regex literal,
    `// ...` and `/* ... */`. A comment opens only in code state, so a quote inside a comment opens no string and
    a `//` inside a string opens no comment. A template literal's `${...}` returns to code state until
    its matching `}`; a stack holds the nesting. This is the defect class ts/CLAUDE.md records for
    `stripComments` (SMA-639), so the self-test covers each state.

    `code` is `text` with each comment replaced by spaces (newlines kept), so every offset stays the
    same. `masked` is `code` with the contents of each string and template literal replaced by `_`
    (quotes kept), so a regex over `masked` never matches inside a string. `literals` maps the offset
    of each `'` or `"` opening quote to (end, value): `end` is the offset after the closing quote.

    A `/` in code state that does not start a comment opens a regex literal when the previous code
    character is one of `( , = : [ ! & | ? { } ; + - * % < > ~ ^` (`>` covers `=>`), when the text
    starts there, or when the previous word is `return`, `typeof`, `in`, `of`, `case`, `void`,
    `delete`, `throw`, `new`, `yield`, `await`, `instanceof`, `else` or `do`. Any other `/` is a
    division. After `++` or `--` a `/` is a division, because a postfix operator ends an operand. A division
    that this rule reads as a regex in a rarer form can still hide an alias with no row. The
    real-corpus pin locks the result for today's configs. A `//` or `/*` right after a backslash in code state is a row. The regex state honours `\\` escapes and `[...]`
    classes (a `/` inside a class does not close it), closes at the next unescaped `/`, and skips the
    flag letters. Its content is masked like a string, so it opens no comment and no string. A newline
    before the close is a row. Without this state, `/^https?:\\/\\//` read as a comment and dropped
    an alias with no row.
    """
    code = list(text)
    masked = list(text)
    literals = {}
    stack = [["code", 0]]
    prev = ""
    i, n = 0, len(text)
    while i < n:
        mode = stack[-1]
        ch = text[i]
        nxt = text[i + 1] if i + 1 < n else ""
        if mode[0] == "tpl":
            if ch == "\\":
                for k in (i, i + 1):
                    if k < n and text[k] != "\n":
                        masked[k] = "_"
                i += 2
            elif ch == "`":
                stack.pop()
                prev = "`"
                i += 1
            elif ch == "$" and nxt == "{":
                stack.append(["code", 0])
                prev = "{"
                i += 2
            else:
                if ch != "\n":
                    masked[i] = "_"
                i += 1
            continue
        if ch == "/" and nxt != "" and nxt in "/*" and text[i - 1:i] == "\\":
            rows.append(f"{where}: a comment marker after a backslash at line {text.count(chr(10), 0, i) + 1}; A13 cannot scan it")
            return None
        if ch == "/" and nxt == "/":
            end = text.find("\n", i)
            end = n if end == -1 else end
            for k in range(i, end):
                code[k] = masked[k] = " "
            i = end
            continue
        if ch == "/" and nxt == "*":
            end = text.find("*/", i + 2)
            if end == -1:
                rows.append(f"{where}: a `/*` comment is not closed, so A13 cannot read the file")
                return None
            for k in range(i, end + 2):
                if text[k] != "\n":
                    code[k] = masked[k] = " "
            i = end + 2
            continue
        if ch in "'\"":
            j, value = i + 1, []
            while j < n and text[j] not in (ch, "\n"):
                if text[j] == "\\" and j + 1 < n:
                    value.append(text[j + 1])
                    j += 2
                    continue
                value.append(text[j])
                j += 1
            if j >= n or text[j] != ch:
                rows.append(f"{where}: a {ch} string is not closed on its line, so A13 cannot read the file")
                return None
            for k in range(i + 1, j):
                masked[k] = "_"
            literals[i] = (j + 1, "".join(value))
            prev = ch
            i = j + 1
            continue
        if ch == "/" and (prev == "" or prev in _JS_REGEX_PREV or _JS_REGEX_KEYWORD_RE.search(text[max(0, i - 16):i])) \
                and not text[max(0, i - 40):i].rstrip().endswith(("++", "--")):
            j, in_class = i + 1, False
            while j < n and text[j] != "\n":
                c = text[j]
                if c == "\\":
                    if text[j + 1:j + 2] == "\n":
                        break
                    j += 2
                    continue
                if in_class:
                    in_class = c != "]"
                elif c == "[":
                    in_class = True
                elif c == "/":
                    break
                j += 1
            if j >= n or text[j] != "/":
                rows.append(f"{where}: an unterminated regex literal at line {text.count(chr(10), 0, i) + 1}, so A13 cannot scan it")
                return None
            j += 1
            while j < n and (text[j].isalpha()):
                j += 1
            for k in range(i + 1, j):
                if text[k] != "\n":
                    masked[k] = "_"
            prev = "/"
            i = j
            continue
        if ch == "`":
            stack.append(["tpl", 0])
        elif ch == "{":
            mode[1] += 1
        elif ch == "}":
            if len(stack) > 1 and mode[1] == 0:
                stack.pop()
            else:
                mode[1] -= 1
        if not ch.isspace():
            prev = ch
        i += 1
    if len(stack) > 1:
        rows.append(f"{where}: a template literal is not closed, so A13 cannot read the file")
        return None
    return "".join(code), "".join(masked), literals


def _matching_close(masked, open_idx):
    """The offset of the bracket that closes the one at `open_idx`, or None if it never closes."""
    depth = 0
    for k in range(open_idx, len(masked)):
        ch = masked[k]
        if ch in _JS_OPEN:
            depth += 1
        elif ch in _JS_CLOSE:
            depth -= 1
            if depth == 0:
                return k
    return None


def _top_level_entries(masked, start, end):
    """The (start, end) spans of the comma-separated entries in masked[start:end], trimmed.

    Only a comma at bracket depth 0 splits. An empty entry is returned as a span with start == end.
    """
    spans, depth, s = [], 0, start
    for k in range(start, end):
        ch = masked[k]
        if ch in _JS_OPEN:
            depth += 1
        elif ch in _JS_CLOSE:
            depth -= 1
        elif ch == "," and depth == 0:
            spans.append((s, k))
            s = k + 1
    spans.append((s, end))
    trimmed = []
    for a, b in spans:
        while a < b and masked[a].isspace():
            a += 1
        while b > a and masked[b - 1].isspace():
            b -= 1
        trimmed.append((a, b))
    return trimmed


def _js_bindings(masked, literals, config_dir):
    """`const` bindings an alias value may name (spec §4.4 step 3).

    `const <id> = '<lit>'` gives ("lit", lit). `const <id> = fileURLToPath(new URL('<lit>',
    import.meta.url))` gives ("path", <lit> resolved against the config's directory). Any other
    `const` is not a binding, so an alias that names it is unresolvable.
    """
    bindings = {}
    for m in _JS_CONST_STRING_RE.finditer(masked):
        lit = literals.get(m.end())
        if lit and _JS_STATEMENT_END_RE.match(masked, lit[0]):
            bindings[m.group(1)] = ("lit", lit[1])
    for m in _JS_CONST_URL_RE.finditer(masked):
        lit = literals.get(m.end())
        if lit is None or not _JS_URL_TAIL_RE.match(masked, lit[0]):
            continue
        if lit[1].startswith("/") or ":" in lit[1]:
            continue
        bindings[m.group(1)] = ("path", os.path.normpath(os.path.join(config_dir, lit[1])))
    return bindings


def _alias_value(masked, literals, bindings, vs, ve):
    """Classify the alias value in masked[vs:ve]: ("lit", s), ("path", p), or (None, None)."""
    if vs < ve and masked[vs] in "'\"" and literals.get(vs, (None,))[0] == ve:
        return ("lit", literals[vs][1])
    ident = masked[vs:ve]
    if _JS_IDENT_RE.fullmatch(ident) and ident in bindings:
        return bindings[ident]
    return (None, None)


def vitest_config_facts(text, config_rel, rows):
    """Return (aliases, tsconfig_off) for one vitest config file (spec §4.4).

    `aliases` is a sorted list of (key, target) pairs, `target` relative to the repository root and
    normalized (it can start with `..` when it leaves the root; the caller reports that). An alias
    whose value is a bare package name adds nothing. `tsconfig_off` is True when the code holds
    `tsconfig: false` as a whole property. Each form outside the grammar appends a row to `rows`.
    """
    scanned = _scan_js(text, config_rel, rows)
    if scanned is None:
        return [], False
    _code, masked, literals = scanned
    config_dir = os.path.dirname(config_rel)
    for m in _JS_IMPORT_RE.finditer(masked):
        lit = literals.get(m.end())
        if lit and lit[1].startswith(("./", "../")):
            rows.append(f"{config_rel}: imports {lit[1]!r}; A13 cannot read a config that is split across files")
    for m in _JS_SPLIT_CONFIG_RE.finditer(masked):
        rows.append(f"{config_rel}: uses `{m.group(1)}`; A13 cannot read a config that is split across files")
    for m in _JS_PROJECTS_RE.finditer(masked):
        start = m.end()
        end = _matching_close(masked, start) if masked[start:start + 1] == "[" else None
        if end is None:
            rows.append(f"{config_rel}: the `projects` value is not an array literal A13 can read")
            continue
        for s, e in _top_level_entries(masked, start + 1, end):
            if s < e and masked[s] in "'\"`":
                rows.append(
                    f"{config_rel}: `projects` holds a string entry; vitest loads it as another config or "
                    f"a glob, which A13 does not follow"
                )
    bindings = _js_bindings(masked, literals, config_dir)
    key_ends = [m.end() for m in _JS_ALIAS_WORD_RE.finditer(masked)]
    key_ends += [end for end, value in literals.values() if value == "alias" and _JS_KEY_COLON_RE.match(masked, end)]
    for off, (end, value) in literals.items():
        if value == "alias" and masked[:off].rstrip().endswith("[") and masked[end:].lstrip().startswith("]"):
            rows.append(f"{config_rel}: a computed key ['alias'] holds an alias object, which A13 cannot read")
    aliases = []
    for key_end in sorted(key_ends):
        colon = _JS_COLON_RE.match(masked, key_end)
        if colon is None:
            rows.append(f"{config_rel}: `alias` is used without a `:` value (a shorthand or a variable), which A13 cannot read")
            continue
        open_idx = colon.end()
        close_idx = _matching_close(masked, open_idx) if masked[open_idx:open_idx + 1] == "{" else None
        if close_idx is None:
            rows.append(f"{config_rel}: an `alias` value is not an object literal, so A13 cannot read it")
            continue
        entries = _top_level_entries(masked, open_idx + 1, close_idx)
        for idx, (s, e) in enumerate(entries):
            if s == e:
                if idx != len(entries) - 1:
                    rows.append(f"{config_rel}: an `alias` object holds an empty entry")
                continue
            if masked.startswith("...", s):
                rows.append(f"{config_rel}: an `alias` object holds a spread entry, which A13 cannot read")
                continue
            if masked[s] == "[":
                rows.append(f"{config_rel}: an `alias` object holds a computed key, which A13 cannot read")
                continue
            if masked[s] in "'\"" and s in literals:
                key_end_idx, key = literals[s]
            else:
                km = _JS_IDENT_RE.match(masked, s)
                if km is None:
                    rows.append(f"{config_rel}: an `alias` entry has a key A13 cannot read")
                    continue
                key, key_end_idx = km.group(0), km.end()
            sep = _JS_COLON_RE.match(masked, key_end_idx)
            if sep is None or sep.end() > e:
                rows.append(f"{config_rel}: the alias {key} has no `:` value, which A13 cannot read")
                continue
            kind, value = _alias_value(masked, literals, bindings, sep.end(), e)
            if kind == "lit" and value.startswith(("./", "../", "/")):
                kind, value = "path", os.path.normpath(os.path.join(config_dir, value))
            if kind == "path":
                aliases.append((key, value))
            elif kind == "lit" and not key.startswith("@paigasus/"):
                continue
            else:
                rows.append(f"{config_rel}: the alias {key} has a value A13 cannot resolve")
    return sorted(set(aliases)), bool(_JS_TSCONFIG_OFF_RE.search(masked))


# `vitest` as a bounded token, optionally behind `pnpm exec`. The boundaries are TSC_TOKEN_RE's, so
# `vitest.config.ts`, `vitest-environment-x` and `@vitest/coverage-v8` never match. A vitest call
# behind a wrapper script is invisible (spec N3); the task floor catches the loss of a known task.
VITEST_TOKEN_RE = re.compile(r"(^|[\s;&|(])(pnpm\s+exec\s+)?vitest(\s|$)")
# vitest 5.0.3's own lookup order when no `--config` is given (spec §2 item 4).
VITEST_CONFIG_NAMES = tuple(f"{stem}.config.{ext}" for stem in ("vitest", "vite") for ext in ("ts", "mts", "cts", "js", "mjs", "cjs"))
_VITEST_SPAN_END_RE = re.compile(r"&&|\|\||;|\||\n")
# `cd` or `pushd` as a command word. `--cwd` is an argument of another tool and does not match.
_VITEST_CD_RE = re.compile(r"(?:^|[\s;&|(])(?:cd|pushd)(?=\s|$)")
# `pnpm -C <dir>`, `pnpm --dir <dir>` and `pnpm --dir=<dir>` change the directory like `cd` does.
_VITEST_PNPM_DIR_RE = re.compile(r"(?:^|[\s;&|(])pnpm\s+(?:[^\s;&|]+\s+)*?(?:-C|--dir)(?:=|\s|$)")
# A balanced `( ... )` group: a subshell or a `$( ... )`. A `cd` inside one ends with the group,
# so it does not move the vitest call that follows. The real paigasus-kernel-ts:test has this shape.
_SUBSHELL_RE = re.compile(r"\([^()]*\)")
_VITEST_CONFIG_BAD_CHARS = frozenset("$*?[]{}'\"`")


def derive_vitest_tasks(projects):
    """Every `<pid>:<task>` of a `language: typescript` project whose resolved invocation runs vitest.

    Raises MoonOutputError if a task exposes none of a command, a script, or any args, exactly as
    derive_tsc_tasks does.
    """
    return _derive_ts_tasks(projects, VITEST_TOKEN_RE, "vitest")


def vitest_invocation_configs(target, blob, root, own, rows):
    """The config files that the vitest calls in `blob` read, as repository-relative paths (spec §4.2).

    For each vitest call, the words after the token up to the next `&&`, `||`, `;`, `|` or newline
    are read. `--config <f>`, `--config=<f>` and `-c <f>` name a config relative to `own`, the task's
    source directory. With no flag, vitest's own lookup applies in `own`; no file there means no
    config, which is not a row. `--root`, `-r`, a `cd` before the call outside a subshell, and a
    config path with `$`, a glob character or a quote are rows, never skips.
    """
    configs = []
    for m in VITEST_TOKEN_RE.finditer(blob):
        prefix = blob[:m.start()]
        while True:
            stripped = _SUBSHELL_RE.sub(" ", prefix)
            if stripped == prefix:
                break
            prefix = stripped
        if _VITEST_CD_RE.search(prefix) or _VITEST_PNPM_DIR_RE.search(prefix):
            rows.append(f"{target} changes directory with `cd` before it runs vitest, so A13 cannot tell which config vitest reads")
            continue
        # The span search starts at the separator group (group 3). A newline after a bare `vitest`
        # token then ends the span, and the words of the next command are not flags of this call.
        end = _VITEST_SPAN_END_RE.search(blob, m.start(3))
        words = blob[m.end():end.start() if end else len(blob)].split()
        explicit, refused = [], False
        i = 0
        while i < len(words):
            word = words[i]
            if word in ("--root", "-r") or word.startswith(("--root=", "-r=")):
                rows.append(f"{target} runs vitest with {word.split('=', 1)[0]}, so A13 cannot tell which config vitest reads")
                refused = True
                break
            if word in ("--config", "-c"):
                if i + 1 == len(words):
                    rows.append(f"{target} runs vitest with {word} and no path")
                    refused = True
                    break
                explicit.append(words[i + 1])
                i += 2
                continue
            if word.startswith(("--config=", "-c=")):
                explicit.append(word.split("=", 1)[1])
            i += 1
        if refused:
            continue
        for raw in explicit:
            rel = os.path.normpath(os.path.join(own, raw))
            if any(c in _VITEST_CONFIG_BAD_CHARS for c in raw) or os.path.isabs(raw) or rel.startswith("../"):
                rows.append(
                    f"{target} runs vitest with the config path {raw}, which A13 cannot resolve (it holds `$`, "
                    f"a glob character or a quote, or it leaves the repository)"
                )
            elif not (root / rel).is_file():
                rows.append(f"{target} runs vitest with {rel}, which does not exist")
            else:
                configs.append(rel)
        if not explicit:
            for name in VITEST_CONFIG_NAMES:
                if (root / own / name).is_file():
                    configs.append(f"{own}/{name}")
                    break
    return list(dict.fromkeys(configs))


TS_LOCKFILE = "ts/pnpm-lock.yaml"
# The gate that reads the vitest configs. A13 asserts that each config it reads matches one of this
# task's inputs, so a config with a new name or in a new place cannot hide from the gate (spec §4.6).
AFFECTED_SMOKE_PROJECT = "repo"
AFFECTED_SMOKE_TASK = "affected-smoke"
_OUT_DIR_RE = re.compile(r"--out-dir(?:=|\s+)([^\s;&|)]+)")

# A13's anti-vacuity floors (spec §4.7). A13 asserts CONTAINMENT, and a containment check whose
# `want` set empties passes having asserted nothing (A7's lesson). The task floor catches a
# derivation that stops matching a known task. The alias floor catches a parser that stops finding
# the kernel's two binding aliases.
REQUIRED_VITEST_TASKS = (
    "paigasus-kernel-ts:test",
    "paigasus-console-core-ts:test",
    "iam-console-ts:test",
    "gateway-console-ts:test",
    "paigasus-auth-ts:test-e2e",
)
REQUIRED_VITEST_ALIASES = {"paigasus-kernel-ts:test": {"@paigasus/node-bindings", "@paigasus/wasm"}}


def _binding_runtime_inputs(root, pkg_dir, data, rows):
    """The files vitest loads from one `file:` binding: its package.json and each `files` entry that
    is not a `.d.ts` (spec §4.3 item 4). vitest runs the glue and the `.wasm`; it never reads a `.d.ts`.
    """
    want = {f"{pkg_dir}/package.json"}
    if "files" not in data:
        rows.append(f"{pkg_dir}/package.json has no `files` key, so A13 cannot tell which files vitest loads from the binding")
        return want
    files = data["files"]
    if not isinstance(files, list):
        rows.append(f"{pkg_dir}/package.json `files` is not a list")
        return want
    for entry in files:
        if not isinstance(entry, str):
            rows.append(f"{pkg_dir}/package.json `files` holds an entry of type {type(entry).__name__}, not a string")
            continue
        if any(c in entry for c in "*?[]{}!"):
            rows.append(f"{pkg_dir}/package.json `files` entry {entry!r} holds a glob character; A13 needs literal entries")
            continue
        rel = os.path.normpath(entry)
        if rel.endswith(".d.ts"):
            continue
        if (root / pkg_dir / rel).is_dir():
            rows.append(f"{pkg_dir}/package.json `files` entry {entry!r} is a directory; A13 needs file entries")
            continue
        want.add(f"{pkg_dir}/{rel}")
    return want


def _tsconfig_chain(root, pkg_dir, rows):
    """`<pkg_dir>/tsconfig.json` and each file its relative `extends` chain reaches (spec §4.3 item 5).

    A missing tsconfig.json adds nothing. The file is JSONC (the real kernel and ui tsconfig files
    hold `//` comments), so `_scan_js` removes the comments before `json.loads`. An `extends` that is
    not a relative path (a package specifier, an array) is a row, never a skip.
    """
    rel = f"{pkg_dir}/tsconfig.json"
    if not (root / rel).is_file():
        return []
    chain = []
    while rel not in chain:
        chain.append(rel)
        try:
            text = _read_utf8(root / rel)
        except (OSError, MoonOutputError) as exc:
            rows.append(f"{rel} cannot be read ({exc})")
            break
        scanned = _scan_js(text, rel, rows)
        if scanned is None:
            break
        try:
            data = json.loads(scanned[0])
        except ValueError as exc:
            rows.append(f"{rel} is not valid JSON after its comments are removed ({exc})")
            break
        if not isinstance(data, dict):
            rows.append(f"{rel} is not a JSON object")
            break
        ext = data.get("extends")
        if ext is None:
            break
        if not isinstance(ext, str) or not ext.startswith(("./", "../")):
            rows.append(f"{rel}: `extends` is {ext!r}; A13 follows only a relative path, so add a rule for this form on purpose (SMA-736)")
            break
        nxt = os.path.normpath(os.path.join(os.path.dirname(rel), ext))
        if not (root / nxt).is_file():
            rows.append(f"{rel}: `extends` points at {nxt}, which does not exist")
            break
        rel = nxt
    return chain


def vitest_required_inputs(root, own, closure, facts, rows):
    """Items 1 to 5 of want(T) for one vitest task (spec §4.3). Item 6 (aliases) is the caller's.

    `facts` is a list of (config, aliases, tsconfig_off), one per config file the task reads. The
    tsconfig files are skipped only when there is at least one config and every config sets
    `tsconfig: false`. With no config file, vitest runs with defaults and loads the tsconfig files.
    """
    want = {TS_LOCKFILE}
    want.update(cfg for cfg, _aliases, _off in facts)
    for name in sorted(closure):
        kind, pkg_dir = closure[name]
        data = _read_package_json(root, pkg_dir, rows)
        if data is None:
            want.add(f"{pkg_dir}/package.json")
            continue
        if kind == "workspace":
            want |= workspace_package_inputs(pkg_dir, data, rows)
        else:
            want |= _binding_runtime_inputs(root, pkg_dir, data, rows)
    if not (facts and all(off for _cfg, _aliases, off in facts)):
        for pkg_dir in [own, *sorted(d for k, d in closure.values() if k == "workspace")]:
            want.update(_tsconfig_chain(root, pkg_dir, rows))
    return want


def _builds_scratch_alias(target, blob, dest, ffi):
    """D3: True when `target` is an FFI task whose invocation names `dest`'s directory as `--out-dir`.

    The `--out-dir` value is relative to a `cd` inside the invocation, so it matches when it equals
    the directory or is a whole-segment suffix of it (`.wasmpack-test-out` matches
    `rs/crates/bindings/paigasus-wasm/.wasmpack-test-out`, `t-out` does not).
    """
    if target not in ffi:
        return False
    out_dir = os.path.dirname(dest)
    for m in _OUT_DIR_RE.finditer(blob):
        value = os.path.normpath(m.group(1).strip("'\""))
        if out_dir == value or out_dir.endswith("/" + value):
            return True
    return False


def smoke_unreached(projects, paths):
    """The `paths` that no input of repo:affected-smoke matches, sorted. None if it reports no `inputGlobs`.

    One match for A13 and for A14's G5 (SMA-768 follow-up), so the two cannot drift. A path is
    reached when it is in the task's `inputFiles`, or when a positive input glob matches it. A
    negated glob (`!x`) removes a path from the inputs, even when a positive glob matches it.
    """
    smoke = projects.get(AFFECTED_SMOKE_PROJECT) or {}
    smoke_globs = (smoke.get("task_input_globs") or {}).get(AFFECTED_SMOKE_TASK)
    if smoke_globs is None:
        return None
    smoke_files = (smoke.get("task_inputs") or {}).get(AFFECTED_SMOKE_TASK) or []
    patterns = [_moon_glob_re(g) for g in smoke_globs if not g.startswith("!")]
    excluded = [_moon_glob_re(g[1:]) for g in smoke_globs if g.startswith("!")]
    out = []
    for path in sorted(paths):
        reached = path in smoke_files or any(p.fullmatch(path) for p in patterns)
        if not reached or any(p.fullmatch(path) for p in excluded):
            out.append(path)
    return out


def _moon_glob_re(glob):
    """A regex for one moon input glob: `**/` and `**` cross `/`, `*` and `?` do not, `{a,b}` is a choice."""
    out, i = [], 0
    while i < len(glob):
        if glob.startswith("**/", i):
            out.append("(?:.*/)?")
            i += 3
        elif glob.startswith("**", i):
            out.append(".*")
            i += 2
        elif glob[i] == "*":
            out.append("[^/]*")
            i += 1
        elif glob[i] == "?":
            out.append("[^/]")
            i += 1
        elif glob[i] == "{" and "}" in glob[i:]:
            end = glob.index("}", i)
            out.append("(?:" + "|".join(re.escape(p) for p in glob[i + 1:end].split(",")) + ")")
            i = end + 1
        else:
            out.append(re.escape(glob[i]))
            i += 1
    return re.compile("".join(out))


def check_ts_vitest_inputs(projects, root, tracked, floor=REQUIRED_VITEST_TASKS, alias_floor=REQUIRED_VITEST_ALIASES):
    """Return the A13 violation list: vitest tasks that do not key on what vitest reads (SMA-736).

    CONTAINMENT, per task, over both input buckets, as A7 and A12a. Over-declaration is allowed. A
    declared glob that covers a wanted literal path does not satisfy it; the literal must be declared.

    `root` and `tracked` are POSITIONAL AND REQUIRED, never defaulted, for the reason in A7's
    docstring (SMA-560 I3). `tracked` is the set of git-tracked paths: an alias target that git does
    not track is valid only under D3 (`_builds_scratch_alias`).

    Row order: floor rows, then walk and parser rows, then per-task rows. A package.json walk row that
    A12a already reports is dropped here, so one broken manifest prints under one title.
    """
    tasks = derive_vitest_tasks(projects)
    ffi = derive_ffi_tasks(projects)
    a12_walk = set(ts_tsc_analysis(projects, root)[3])
    floor_rows, walk_rows, task_rows = [], [], []
    names = ts_workspace_packages(root, walk_rows)
    configs_read = set()
    task_aliases = {}
    for target in sorted(tasks):
        pid, _, task = target.partition(":")
        proj = projects[pid]
        own = proj["source_dir"]
        blob = proj["invocations"][task]
        own_pkg = _read_package_json(root, own, walk_rows)
        own_name = own_pkg.get("name") if own_pkg else None
        closure = ts_closure(root, own, names, walk_rows)
        facts = []
        for cfg in vitest_invocation_configs(target, blob, root, own, task_rows):
            try:
                text = _read_utf8(root / cfg)
            except (OSError, MoonOutputError) as exc:
                walk_rows.append(f"{cfg} cannot be read ({exc})")
                continue
            aliases, tsconfig_off = vitest_config_facts(text, cfg, walk_rows)
            facts.append((cfg, aliases, tsconfig_off))
            configs_read.add(cfg)
        task_aliases[target] = {key for _cfg, aliases, _off in facts for key, _dest in aliases}
        want = vitest_required_inputs(root, own, closure, facts, walk_rows)
        for cfg, aliases, _off in facts:
            for key, dest in aliases:
                if dest == own or dest.startswith(own + "/"):
                    continue
                if dest == ".." or dest.startswith("../") or os.path.isabs(dest):
                    task_rows.append(f"{target} aliases {key} in {cfg} to {dest}, which is outside the repository")
                    continue
                if key.startswith("@paigasus/") and key not in closure and key != own_name:
                    task_rows.append(f"{target} aliases {key} in {cfg}, but {key} is not in its package.json closure")
                    continue
                if dest in tracked:
                    want.add(dest)
                elif not _builds_scratch_alias(target, blob, dest, ffi):
                    task_rows.append(
                        f"{target} aliases {key} to the untracked path {dest}, but the task does not build it "
                        f"there (no FFI build with --out-dir {os.path.dirname(dest)})"
                    )
        files = (proj.get("task_inputs") or {}).get(task)
        globs = (proj.get("task_input_globs") or {}).get(task)
        if files is None or globs is None:
            task_rows.append(
                f"{target} reported no `inputFiles`/`inputGlobs` — moon's output shape changed, so "
                f"this assertion cannot be evaluated (treated as a violation, never skipped)"
            )
        else:
            observed = set(files) | set(globs)
            for entry in sorted(want - observed):
                task_rows.append(f"{target} inputs omit {entry}")
        deps = (proj.get("tasks") or {}).get(task) or []
        if (own_name == PROTO_PACKAGE or PROTO_PACKAGE in closure) and PROTO_GENERATE_DEP not in deps:
            task_rows.append(
                f"{target} deps omit {PROTO_GENERATE_DEP} — it reads @paigasus/proto's generated "
                f"tree, so it must run after the generator for a deterministic cache key"
            )
    if configs_read:
        unreached = smoke_unreached(projects, configs_read)
        if unreached is None:
            task_rows.append(
                f"{AFFECTED_SMOKE_PROJECT}:{AFFECTED_SMOKE_TASK} reported no `inputGlobs`, so A13 cannot check "
                f"that an edit to a vitest config schedules it"
            )
        else:
            for cfg in unreached:
                task_rows.append(
                    f"{cfg} matches no input of {AFFECTED_SMOKE_PROJECT}:{AFFECTED_SMOKE_TASK}, so an edit to it "
                    f"alone does not schedule the gate that reads it"
                )
    for target in sorted(set(floor or ()) - tasks):
        floor_rows.append(
            f"FLOOR: {target} is not matched by the vitest token, so A13 asserts nothing about it "
            f"(see VITEST_TOKEN_RE; a vitest call behind a wrapper script is invisible)"
        )
    for target, keys in sorted((alias_floor or {}).items()):
        for key in sorted(set(keys) - task_aliases.get(target, set())):
            floor_rows.append(
                f"FLOOR: {target} no longer aliases {key} in a config A13 parses, so the alias rule "
                f"asserts nothing for it"
            )
    walk_rows = [row for row in walk_rows if row not in a12_walk]
    return list(dict.fromkeys(floor_rows + walk_rows + task_rows))


# SMA-765, SMA-768 (A14) - a Moon task must never change a uv project environment IN PLACE. Two
# tasks that share py/.venv race: an exact `uv sync` in one removes the packages another one imports.
#
# An allow-list, not a denylist (SMA-768): every `uv` token in a scanned file must be one of the
# approved forms below, or the line is a row. A new form is one edit to these constants plus a
# self-test row. The rule is in the spec docs/superpowers/specs/2026-11-13-sma-768-a14-uv-gate-design.md.
UV_TOKEN_RE = re.compile(r"(?<![\w.-])uv(?![\w.-])")
UV_APPROVED_FLAGS = frozenset(("--locked", "--frozen", "--isolated"))
# An approved option with a value. The value is the next word, compared as exact text.
UV_APPROVED_VALUES = {
    "--project": ("py", "ci/release-plan", '"$HERE"'),
    "--python": ("'>=3.12'",),
}
UV_BINDINGS_REINSTALL = ("--reinstall-package", "paigasus-py-bindings")
UV_COMMAND_WORD_RE = re.compile(r"[A-Za-z0-9_][A-Za-z0-9_.+/-]*")
UV_REDIRECT_WORDS = (">/dev/null", "2>/dev/null", "2>&1")
# Rule 6: a name that selects or changes the uv environment. Any mention is a row.
UV_ENV_NAME_RE = re.compile(r"(?<![\w])(?:UV_[A-Z0-9_]+|VIRTUAL_ENV)(?![\w])|(?<![\w-])--active(?![\w-])")
# One invocation ends at the first of these. The cut is not quote-aware, on purpose (R6). It cannot
# change a verdict today: no approved option value and no approved flag holds a separator, so a cut
# inside a quoted value leaves a value that is not exact text, which is red.
UV_CUT_RE = re.compile(r"&&|\|\||[;|)`]")
UV_PROBE_RE = re.compile(r"(?<![\w.-])command\s+-v\s+$")
# R2 (follow-up sweep 2): the probe inside `$( … )` or backticks captures the uv path, so a later
# call through the variable is not seen. A row.
UV_PROBE_CAPTURE_RE = re.compile(r"(?:\$\(|`)\s*command\s+-v\s+$")
# R-relock (follow-up sweep 2): a `uv run` needs one of these. Without one, uv can relock the
# project and then change the environment to match the new lock while another task reads it.
UV_RUN_LOCK_FLAGS = ("--locked", "--frozen", "--isolated")
# R3 (follow-up sweep 2): a command word that `uv run` runs and that installs into the environment.
UV_RUN_INSTALLERS = frozenset(("pip", "pip3", "uv", "ensurepip", "virtualenv"))
# `pip3.12` and other version-qualified pip names (CodeRabbit, PR 461).
UV_PIP_VERSIONED_RE = re.compile(r"pip[0-9]*(?:\.[0-9]+)*")
# R2 (follow-up sweep 2): spellings that a shell turns into `uv` but the token regex cannot see.
# `eval` runs text that A14 cannot read. `${x}v`, `$(…)v` and `u$x` join `uv` from an expansion.
UV_EVAL_RE = re.compile(r"(?<![\w.-])eval(?![\w.-])")
UV_SPLICE_RE = re.compile(r"[})`]v(?![\w.-])|(?<![\w.$-])u(?=\$[{(A-Za-z_]|`)")
UV_QUOTE_CHARS_RE = re.compile(r"[\\'\"]")
# R4 (follow-up sweep 2): a write to an environment that does not go through `uv`.
UV_DIRECT_WRITE_RE = re.compile(
    r"(?<![\w.-])pip[0-9]*(?:\.[0-9]+)*(?![\w.-])[^;&|\n]*?(?<![\w.-])(?:install|uninstall|sync)(?![\w-])"
    r"|(?<![\w-])-m\s*(?:pip|ensurepip|venv|virtualenv)(?![\w-])"
    r"|\.venv[\w./-]*/bin/(?:pip3?|uv|python[0-9.]*\s+-m\s*(?:pip|ensurepip))(?![\w-])"
    r"|(?<![\w.-])(?:rm|mv|cp|ln|rsync)(?![\w.-])[^;&|\n]*\.venv(?![\w-])"
)


def _uv_logical_lines(text):
    """Yield `(logical, starts)` for each non-comment logical line of `text`.

    A physical line whose first non-blank character is `#` is a comment: it is skipped and never
    joins the next line. A non-comment line that ends in a backslash joins the next physical line with no
    separator, as bash does, so a continuation can split a word.
    `starts` lists `(offset in logical, physical line number)` for each joined line.
    """
    lines = text.split("\n")
    i, n, quote = 0, len(lines), None
    while i < n:
        # R1 (follow-up sweep 2): a `#` line is a comment only when no quote is open from an
        # earlier line. Inside `bash -c '` + newline + `#'; uv sync`, bash runs the text after `'`.
        if (quote is None or quote[-1] == "$(") and lines[i].lstrip(" \t").startswith("#"):
            i += 1
            continue
        parts, starts, pos = [], [], 0
        while i < n:
            cont = lines[i].endswith("\\")
            body = lines[i][:-1] if cont else lines[i]
            starts.append((pos, i + 1))
            parts.append(body)
            pos += len(body)
            i += 1
            if not cont:
                break
        # Over the JOINED line, so a word that a continuation splits stays one word (`a\`+newline+`#`).
        quote = _uv_quote_after("".join(parts), quote)
        yield "".join(parts), starts


def _uv_quote_after(line, quote):
    """The context stack open at the end of `line`, given the stack `quote` at its start (R1).

    The stack holds `'`, `"` and `$(`. Inside `"…"`, a `$(` starts a new context in which quotes
    nest again, as in bash. A `#` that starts a word outside a quote ends the scan: the rest of
    the line is a comment. Returns None when no context is open.
    """
    stack = list(quote or ())
    i, n, word_start = 0, len(line), True
    while i < n:
        c, top = line[i], (stack[-1] if stack else None)
        # A shell operator also ends a word, so `true;# '` starts a comment at the `#`.
        at_word_start, word_start = word_start, c in " \t;&|()<>"
        if top == "'":
            if c == "'":
                stack.pop()
        elif c == "\\":
            # An escaped character is a word character, so a `#` after `\ ` is mid-word.
            i += 2
            continue
        elif top == '"':
            if c == '"':
                stack.pop()
            elif line.startswith("$(", i):
                stack.append("$(")
                i += 2
                continue
        elif c in "'\"":
            stack.append(c)
        elif line.startswith("$(", i):
            stack.append("$(")
            i += 2
            continue
        elif c == ")" and top == "$(":
            stack.pop()
        elif c == "#" and at_word_start:
            break
        i += 1
    return tuple(stack) or None


def _uv_bash_scan(text):
    """Scan `text` once as bash does. Return `(eff, live)`. Only the floor uses it.

    `eff` is the text with every comment and every removed `\\`+newline taken out. `live[i]` is True
    when `eff[i]` is outside any quote. A comment starts at an unquoted `#` that starts a word and
    ends at the next physical newline. `\\`+newline is removed outside a comment and outside single
    quotes, so a `\\` that ends a comment line does NOT join the next line.
    """
    eff, live = [], []
    quote, comment, i, n, escaped_end = None, False, 0, len(text), -1
    while i < n:
        ch = text[i]
        if comment:
            if ch == "\n":
                comment = False
                eff.append(ch)
                live.append(True)
            i += 1
            continue
        if ch == "\\" and quote != "'":
            if text[i + 1:i + 2] == "\n":
                i += 2
                continue
            nxt = text[i + 1:i + 2]
            if quote == '"' and nxt not in ('"', "\\", "$", "`"):
                # Inside double quotes `\` escapes only `"`, `\`, `$`, a backtick and newline.
                eff.append(ch)
                live.append(False)
                i += 1
                continue
            eff.append(ch)
            live.append(quote is None)
            if nxt:
                eff.append(nxt)
                live.append(quote is None)
                # An escaped character is a word character, so a `#` after it is mid-word.
                escaped_end = len(eff)
            i += 2
            continue
        if quote is None and ch == "#" and len(eff) != escaped_end and (not eff or eff[-1] in " \t\n"):
            comment = True
            i += 1
            continue
        was_open = quote is not None
        if quote:
            if ch == quote:
                quote = None
        elif ch in "'\"":
            quote = ch
        eff.append(ch)
        live.append(not was_open and quote is None)
        i += 1
    return "".join(eff), live


def _uv_words_after(logical, match):
    """The whitespace-split words of the invocation that starts after the `uv` at `match`."""
    tail = logical[match.end():]
    cut = UV_CUT_RE.search(tail)
    return (tail[:cut.start()] if cut else tail).split()


def _uv_run_form(words):
    """Parse `words` that start with `run`. Return `(options, command words, reason or None)`."""
    opts, i = [], 1
    while i < len(words) and words[i].startswith("-"):
        word = words[i]
        if word in UV_APPROVED_FLAGS:
            opts.append(word)
            i += 1
        elif word in UV_APPROVED_VALUES:
            if words[i + 1:i + 2] and words[i + 1] in UV_APPROVED_VALUES[word]:
                opts.append(word)
                i += 2
            else:
                return opts, [], f"{word} needs one of {', '.join(UV_APPROVED_VALUES[word])}"
        elif word == UV_BINDINGS_REINSTALL[0] and words[i + 1:i + 2] == [UV_BINDINGS_REINSTALL[1]]:
            opts.append(word)
            i += 2
        else:
            return opts, [], f"the option {word} is not approved"
    if UV_BINDINGS_REINSTALL[0] in opts and "--isolated" not in opts:
        return opts, [], f"{UV_BINDINGS_REINSTALL[0]} needs --isolated"
    command = words[i:]
    if not command:
        return opts, [], "there is no command word after the options"
    if not UV_COMMAND_WORD_RE.fullmatch(command[0]):
        return opts, command, f"the command word {command[0]!r} is not a bare literal"
    if not any(flag in opts for flag in UV_RUN_LOCK_FLAGS):
        return opts, command, (
            f"`uv run` needs one of {', '.join(UV_RUN_LOCK_FLAGS)}, or it can relock the project and "
            f"change the environment while another task reads it (SMA-765 R-relock)"
        )
    word = command[0].rsplit("/", 1)[-1]
    if word in UV_RUN_INSTALLERS or UV_PIP_VERSIONED_RE.fullmatch(word):
        return opts, command, f"`uv run` runs the installer {command[0]!r} in the environment (R3)"
    # `uv run python -m pip|ensurepip|venv|virtualenv` (with any interpreter options before `-m`)
    # is a row through UV_DIRECT_WRITE_RE, which searches the whole line, and `-m uv` through the
    # `uv` token rule. A second check here could not fail a test.
    return opts, command, None


def _uv_reason(logical, match):
    """None when the `uv` at `match` is an approved form, else the reason it is not."""
    words = _uv_words_after(logical, match)
    if UV_PROBE_RE.search(logical[:match.start()]):
        if UV_PROBE_CAPTURE_RE.search(logical[:match.start()]):
            return "`command -v uv` inside a command substitution captures the uv path, so a call through it is not seen (R2)"
        if all(w in UV_REDIRECT_WORDS for w in words):
            return None
        return "`command -v uv` takes only a redirection after it"
    if words == ["build"]:
        return None
    if words[:1] == ["run"]:
        return _uv_run_form(words)[2]
    if words[:1] == ["build"]:
        return "`uv build` takes no other word"
    return f"the verb {words[0]!r} is not approved" if words else "there is no verb"


def _uv_spelled_tokens(logical):
    """Offsets in `logical` of each `uv` token that exists only after `\\`, `'` and `"` are removed (R2).

    Per token, not per count: a literal `uv` that the removal joins into a longer word must not
    cancel a `u''v` elsewhere on the line. A token of the stripped text is plain only when the raw
    text holds a `uv` token at the same place with no removed character inside it.
    """
    kept = [i for i, c in enumerate(logical) if not UV_QUOTE_CHARS_RE.match(c)]
    stripped = "".join(logical[i] for i in kept)
    raw_tokens = {m.start() for m in UV_TOKEN_RE.finditer(logical)}
    out = []
    for m in UV_TOKEN_RE.finditer(stripped):
        start, end = kept[m.start()], kept[m.end() - 1] + 1
        if not (end - start == 2 and start in raw_tokens):
            out.append(start)
    return out


def uv_form_hits(text):
    """Sorted `(physical line, reason)` for each `uv` token or environment name that is red.

    Strings are NOT stripped (fail-closed): `echo "run uv sync"` is a hit, and a script line that
    only prints a remedy is waived in ALLOW_UV_ENV_SCRIPT instead.
    """
    hits = []
    for logical, starts in _uv_logical_lines(text):
        def line_of(idx, starts=starts):
            return [ln for off, ln in starts if off <= idx][-1]

        for m in UV_TOKEN_RE.finditer(logical):
            why = _uv_reason(logical, m)
            if why:
                hits.append((line_of(m.start()), f"uses `uv` in a form that is not approved: {why}"))
        for m in UV_ENV_NAME_RE.finditer(logical):
            hits.append((line_of(m.start()), f"names {m.group(0)}, which can change the uv environment"))
        # R2: text that a shell can turn into a `uv` call that the token regex does not see.
        for m in UV_EVAL_RE.finditer(logical):
            hits.append((line_of(m.start()), "runs `eval`, so A14 cannot read the command it runs (R2)"))
        for m in UV_SPLICE_RE.finditer(logical):
            hits.append((line_of(m.start()), "joins `u` or `v` to an expansion, which can spell `uv` (R2)"))
        for idx in _uv_spelled_tokens(logical):
            hits.append((line_of(idx), "spells `uv` through quote or backslash removal (R2)"))
        # R4: a write to an environment that does not go through `uv`.
        for m in UV_DIRECT_WRITE_RE.finditer(logical):
            hits.append((line_of(m.start()), f"writes an environment directly ({m.group(0).strip()!r}) (R4)"))
    return sorted(hits)


def uv_mutations(text):
    """One reason per red `uv` token or environment name in `text`; an empty list means clean."""
    return [reason for _, reason in uv_form_hits(text)]


def _floor_isolated(blob):
    """True when ONE uv invocation has `run`, `--isolated`, the bindings reinstall and runs pytest.

    The invocation that is isolated must be the one that runs the tests: a second, ordinary
    `uv run pytest` would run them in the shared environment. The `uv` must start a line or follow
    `&&` or `;`, outside a quote and a comment. `_uv_bash_scan` reads the raw blob once, as bash
    does. A wrong answer of the scan removes credit only.
    """
    eff, live = _uv_bash_scan(blob or "")
    base = 0
    for line in eff.split("\n"):
        for m in UV_TOKEN_RE.finditer(line):
            before = line[:m.start()].rstrip()
            if before and not before.endswith(("&&", ";")):
                continue
            if not live[base + m.start()]:
                continue
            words = _uv_words_after(line, m)
            if words[:1] != ["run"]:
                continue
            opts, command, why = _uv_run_form(words)
            if why or "--isolated" not in opts or UV_BINDINGS_REINSTALL[0] not in opts:
                continue
            if command[:1] == ["pytest"] or command[:3] in (["python", "-m", "pytest"], ["python3", "-m", "pytest"]):
                return True
        base += len(line) + 1
    return False


# A14 follows a `source` / `.` word after `;`, `&`, `|`, `(`, `then`, `do` or `else` too. A8 and A10
# keep SOURCE_STMT_RE (line start only): their measured edge counts depend on it (SMA-605).
UV_SOURCE_STMT_RE = re.compile(
    r"""(?m)(?:^|[;&|({!]|\b(?:then|do|else)\b)\s*(?:source|\.)\s+(?!as\s+\$[A-Za-z_])["']?([^"'\s;&|)]+)["']?"""
)
# `(?!as\s+\$[A-Za-z_])` skips the jq idiom `else . as $rows` (ci/moon-diagnosis/run.sh:278), which is not a source.


# R8 (follow-up sweep 2): a `*.sh` word in a scanned script's executable text. The script can run
# it (`bash ci/release-plan/run.sh`, `"$HERE/x.sh"`), so A14 scans it too. The match is wide on
# purpose: a path in a message is followed as well, which can only add rows.
UV_RUN_REF_RE = re.compile(r"(?:\$\{?[A-Za-z_]\w*\}?/)*((?:[\w.-]+/)*[\w.-]+\.sh)(?![\w.-])")
# The same word WITH its `$NAME/` parts, and a `NAME=value` assignment, so that a directory
# variable that the script assigns is expanded too (CodeRabbit, PR 461).
UV_RUN_REF_FULL_RE = re.compile(r"((?:\$\{?[A-Za-z_]\w*\}?/)+(?:[\w.-]+/)*[\w.-]+\.sh)(?![\w.-])")
UV_RUN_ASSIGN_RE = re.compile(r"""(?m)(?:^|[;&|]\s*)\s*(?:export\s+)?([A-Za-z_]\w*)=["']?([\w./-]+)["']?(?=\s|;|$)""")


def _uv_run_refs(path, root, bases=()):
    """The existing repo files that a `*.sh` word in `path` can name: against the repo root, the
    script's own directory and each of `bases` (the working directory that the task gives the
    script), after the leading `$VAR/` parts are dropped. A word that resolves to no file is
    skipped: bash cannot run a missing file either.
    """
    root_resolved = Path(root).resolve()
    text = _executable_text(path)
    rels = set(UV_RUN_REF_RE.findall(text))
    # A `$NAME/` part with a value that the script assigns (`D=ops; bash "$D/y.sh"`) is also
    # read with EVERY assigned value, so the dropped form above is not the only candidate.
    values = {}
    for name, value in UV_RUN_ASSIGN_RE.findall(text):
        values.setdefault(name, set()).add(value)
    for full in UV_RUN_REF_FULL_RE.findall(text):
        rels.update(c for c in _expand_words(full, values) if "$" not in c)
    out = []
    for rel in sorted(rels):
        for base in (root_resolved, Path(path).resolve().parent, *bases):
            cand = (base / rel).resolve()
            if cand.is_file() and root_resolved in cand.parents:
                out.append(cand)
    return out


def _uv_script_closure(path, root, bases=()):
    """`path` and every script it reaches through `source` / `.` or runs (R8), RESOLVED, cycle-guarded.

    `bases` are the working directories that the task gives its scripts (its project directory).
    A child inherits the cwd, so `bash scripts/x.sh` inside a script also resolves against them.

    `script_source_refs` raises MoonOutputError on a sourced path that resolves to no file, so a
    rename cannot shrink the scan in silence. A run reference that resolves to no file is skipped
    (`_uv_run_refs`).
    """
    queue, seen, out = [Path(path).resolve()], set(), []
    while queue:
        key = queue.pop(0)
        if key in seen:
            continue
        seen.add(key)
        out.append(key)
        queue.extend(x.resolve() for x in script_source_refs(key, root, UV_SOURCE_STMT_RE))
        queue.extend(_uv_run_refs(key, root, bases))
    return out


LOCKSTEP_SCRIPT = "ci/version-lockstep/run.sh"
LOCKSTEP_WRITE_RE = re.compile(r"(?<![\w-])--write(?![\w-])")
# R10 (follow-up sweep 2): `moon run repo:version-lockstep -- --write` passes `--write` through Moon.
LOCKSTEP_MOON_TARGET_RE = re.compile(r"(?:^|:)version-lockstep$")
LOCKSTEP_ASSIGN_RE = re.compile(r"""\s*(?:export\s+)*([A-Za-z_][A-Za-z0-9_]*)=["']?([^"'\s;&|()]+)""")


def _split_commands(text):
    """`text` cut at `&&`, `||`, `;`, `|` and a newline that are outside single and double quotes.

    Not a shell parser (SMA-768 R10 follow-up): it knows quotes and a backslash inside double
    quotes, and nothing else. Remove each backslash-newline continuation before the call.
    """
    out, cur, quote, i = [], [], None, 0
    while i < len(text):
        c = text[i]
        if quote:
            if c == "\\" and quote == '"' and i + 1 < len(text):
                cur.append(text[i:i + 2])
                i += 2
                continue
            if c == quote:
                quote = None
            cur.append(c)
            i += 1
            continue
        if c in "'\"":
            quote = c
        elif text.startswith(("&&", "||"), i):
            out.append("".join(cur))
            cur = []
            i += 2
            continue
        elif c in ";|\n":
            out.append("".join(cur))
            cur = []
            i += 1
            continue
        cur.append(c)
        i += 1
    out.append("".join(cur))
    return out


def _expand_words(word, values):
    """Every spelling of `word` after `$NAME` / `${NAME}` is replaced by each value assigned to NAME."""
    out = {word}
    for name in sorted(values, key=lambda n: (-len(n), n)):
        nxt = set()
        for cand in out:
            if f"${name}" in cand or f"${{{name}}}" in cand:
                for value in values[name]:
                    nxt.add(cand.replace(f"${{{name}}}", value).replace(f"${name}", value))
            else:
                nxt.add(cand)
        out = nxt
    return out


def runs_lockstep_write(blob):
    """True when `blob` can run `ci/version-lockstep/run.sh` with `--write` (A14 rule 8).

    Closed forms (SMA-768 R10 follow-up): the literal path anywhere in a command, a path relative
    to a `cd` or `pushd` in an earlier command (`cd ci/version-lockstep && ./run.sh --write`), a
    path through a `NAME=value` assignment in the blob (`$D/run.sh`), and a quoted separator
    before `--write`. The check over-approximates on purpose, so it can add a row but never hide
    one. It keeps EVERY directory that an earlier `cd` or `pushd` can reach, from the repository
    root on, and never drops one (a `( … )` group, `popd`, or a `cd "$ROOT"` adds nothing and
    removes nothing). It expands a `$NAME` with EVERY value that the blob assigns to NAME, in any
    order. A repeated first word (`cd cd x`, the shape of moon's joined `command` + `script`) is
    read once.
    Not closed: `--write` from a variable, from `"$@"` or from moon passthrough arguments, and a
    script path that only a real shell can compute. See R10 in ci/affected-graph/README.md.
    """
    text = blob.replace("\\\n", "")
    segments = _split_commands(text)
    values = {}
    # Per command, so a value stops at a separator (`D=x; …` assigns `x`, not `x;`).
    for seg in segments:
        m = LOCKSTEP_ASSIGN_RE.match(seg)
        if m:
            values.setdefault(m.group(1), set()).add(m.group(2))
    cwds = {""}
    for seg in segments:
        words = [w.lstrip("(").rstrip(")").strip("'\"") for w in seg.split()]
        words = [w for w in words if w not in ("", "{", "}", "!")]
        if len(words) > 1 and words[0] == words[1]:
            words = words[1:]
        if not words:
            continue
        if words[0] in ("cd", "pushd", "popd"):
            arg = words[1] if len(words) > 1 and words[0] != "popd" else ""
            for cand in _expand_words(arg, values):
                if cand and not cand.startswith(("$", "/", "~", "-")):
                    cwds |= {os.path.normpath(os.path.join(c, cand)) for c in cwds}
            continue
        # Follow-up sweep 2 (R10): every later word is read after expansion. `W=--write; run.sh $W`
        # is a hit. A later word that still holds `$` or a backtick after expansion (`"$@"`, `$*`,
        # an unassigned `$W`, `$(…)`) can be `--write`, so it is a hit too.
        def can_write(rest):
            for w in rest:
                for cand in _expand_words(w, values):
                    if LOCKSTEP_WRITE_RE.search(cand) or "$" in cand or "`" in cand:
                        return True
            return False

        for idx, word in enumerate(words):
            hit = False
            for cand in _expand_words(word, values):
                if LOCKSTEP_SCRIPT in cand or LOCKSTEP_MOON_TARGET_RE.search(cand):
                    hit = True
                elif "$(" in cand or "`" in cand or UV_EVAL_RE.fullmatch(cand):
                    # A path or a command that only a shell can compute can be the lockstep script.
                    # Only a literal `--write` after it is a hit, so a computed path alone is not.
                    if any(LOCKSTEP_WRITE_RE.search(c) for w in words[idx + 1:] for c in _expand_words(w, values)):
                        return True
                elif cand.endswith("run.sh") and not cand.startswith(("$", "/", "~")):
                    hit = hit or any(os.path.normpath(os.path.join(c, cand)) == LOCKSTEP_SCRIPT for c in cwds)
            if hit and can_write(words[idx + 1:]):
                return True
    return False


# R7 (follow-up sweep 2): uv settings that make an approved plain `uv run` reinstall, upgrade or
# rebuild a package in place. They are rows in `[tool.uv]` (and `[tool.uv.pip]`) of a
# pyproject.toml and at the top level (and `[pip]`) of a uv.toml. A user-level uv.toml outside the
# repository cannot be read, and is not covered.
UV_CONFIG_MUTATING_KEYS = ("reinstall", "reinstall-package", "upgrade", "upgrade-package", "cache-keys")
MOON_TOOLCHAINS = ".moon/toolchains.yml"
# The floor of the R7 scan: a list without the py workspace root means the scan read the wrong set.
REQUIRED_UV_CONFIG = "py/pyproject.toml"
# The Moon uv toolchain appends these to its own `uv sync` / `uv venv` (InstallDependencies). Any
# value is a row: the measured command is `uv sync --no-managed-python --no-python-downloads
# --no-progress`, and an added argument such as `--reinstall` would change py/.venv on every run.
# The key anywhere outside a `#` comment, quoted or not, so a flow mapping
# (`unstable_uv: {syncArgs: [...]}`) and a quoted key (`"syncArgs": [...]`) are rows too.
MOON_UV_ARGS_RE = re.compile(r"""(?<![\w-])["']?(syncArgs|venvArgs|installArgs)["']?[ \t]*:""")


def uv_configs_of(tracked):
    """The tracked `pyproject.toml` and `uv.toml` paths in `tracked`, sorted (R7)."""
    return sorted(p for p in tracked if p.rsplit("/", 1)[-1] in ("pyproject.toml", "uv.toml"))


def uv_config_rows(rel_path, text):
    """A14 rows for the mutating uv keys in one pyproject.toml or uv.toml (R7)."""
    try:
        doc = tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        raise MoonOutputError(f"{rel_path}: not valid TOML, so A14 cannot read its uv settings: {exc}") from exc
    if rel_path == "uv.toml" or rel_path.endswith("/uv.toml"):
        tables = [("", doc), ("pip", doc.get("pip"))]
    else:
        uv = (doc.get("tool") or {}).get("uv")
        tables = [("tool.uv", uv), ("tool.uv.pip", uv.get("pip") if isinstance(uv, dict) else None)]
    rows = []
    for name, table in tables:
        if table is None:
            continue
        if not isinstance(table, dict):
            rows.append(f"{rel_path}: [{name}] is not a table, so A14 cannot read it")
            continue
        for key in UV_CONFIG_MUTATING_KEYS:
            if key in table:
                where = f"[{name}] " if name else ""
                rows.append(
                    f"{rel_path}: {where}{key} is set, so a plain `uv run` can change the environment in place (R7)"
                )
    return rows


def _yaml_code(line):
    """`line` without its YAML comment. A comment starts at a `#` that begins the line or follows
    white space, OUTSIDE a quote: a `#` inside `' … '` or `" … "` is part of the value. In a
    double-quoted value `\\` escapes the next character. A single-quoted `''` closes and opens
    the quote again, which gives the same state, so it needs no own branch.
    """
    quote, i = None, 0
    while i < len(line):
        c = line[i]
        if quote == "'":
            if c == "'":
                quote = None
        elif quote == '"':
            if c == "\\":
                i += 2
                continue
            if c == '"':
                quote = None
        elif c in "'\"":
            quote = c
        elif c == "#" and (i == 0 or line[i - 1] in " \t"):
            return line[:i]
        i += 1
    return line


def moon_toolchain_rows(rel_path, text):
    """A14 rows for an argument that Moon appends to its own `uv sync` or `uv venv` (R5, R-toolchain)."""
    rows = []
    for lineno, line in enumerate(text.split("\n"), 1):
        code = _yaml_code(line)
        rows.extend(
            f"{rel_path}:{lineno} sets {m.group(1)}, which Moon appends to the `uv sync` of "
            f"InstallDependencies; A14 approves no argument there (SMA-765 R-toolchain)"
            for m in MOON_UV_ARGS_RE.finditer(code)
        )
    return rows


def check_uv_env_isolation(projects, root, allow=None, floor=None, uv_configs=None, moon_toolchains=None):
    """Return the A14 violation list: tasks that can change a uv environment in place (SMA-765).

    Scanned per task: its resolved blob, the `ci/**/*.sh` scripts it runs (with their `source`
    closure), and the project-relative `scripts/*.sh` files it names. Absent data is a violation:
    an unreadable script raises MoonOutputError, a missing floor task is a `FLOOR:` row. Script
    lines pass an allow-list of approved `uv` forms (`uv_form_hits`); any other `uv` line is a row.
    Rule 7: a task `env:` key that is VIRTUAL_ENV or starts with `UV_` is a row. Rule 8: a task
    that can run `ci/version-lockstep/run.sh` with `--write` (`runs_lockstep_write`) is a row,
    because that runs `uv lock`. An allowlist entry with an empty reason, or one that matches no
    scanned line, is a row, so the list cannot rot. Direction of failure: a row means a task can
    race another task on the shared environment.
    """
    allow = ALLOW_UV_ENV_SCRIPT if allow is None else allow
    floor = REQUIRED_UV_ISOLATED_TASKS if floor is None else floor
    root_resolved = Path(root).resolve()
    rows, matched, hit_lines = [], set(), {}
    reads, scanned = set(), False

    for key, reason in sorted(allow.items()):
        if not str(reason).strip():
            rows.append(f"A14 allowlist entry {key[0]}: {key[1]!r} has an empty reason")

    for pid in sorted(projects):
        proj = projects[pid]
        for name in sorted(proj.get("invocations") or {}):
            target = f"{pid}:{name}"
            blob = proj["invocations"][name]
            if not blob:
                continue
            scanned = True
            for env_name in sorted(dict((proj.get("task_env") or {}).get(name) or {})):
                if env_name == "VIRTUAL_ENV" or env_name.startswith("UV_"):
                    rows.append(f"{target}: the task `env` sets {env_name}")
            # Rule 8: `version-lockstep --write` runs `uv lock`; the EXECUTED waiver assumes no task does.
            if runs_lockstep_write(blob):
                rows.append(
                    f"{target}: runs ci/version-lockstep/run.sh with --write, which runs `uv lock` "
                    f"(the EXECUTED waiver in ALLOW_UV_ENV_SCRIPT assumes no Moon task does)"
                )
            for desc in uv_mutations(blob):
                rows.append(f"{target}: its command {desc}")
            # A14's own closure (UV_SOURCE_STMT_RE), not `task_script_closure`: that one follows a
            # `source` at line start only, and A8 and A10 depend on it as it is.
            paths = []
            # A script inherits the task's cwd (the project directory), so a child script that it runs
            # by a relative path also resolves there (R8).
            task_bases = ((root_resolved / proj["source_dir"]).resolve(),)
            for ref in task_script_refs(projects, root, target):
                paths.extend(_uv_script_closure(ref, root_resolved, task_bases))
            for rel in sorted(set(PROJECT_SCRIPT_REF_RE.findall(blob))):
                path = (root_resolved / proj["source_dir"] / rel).resolve()
                if root_resolved not in path.parents:
                    raise MoonOutputError(
                        f"{target} runs {rel}, which resolves outside the repository - A14 "
                        f"would silently scan less. If the script moved, update the task."
                    )
                if not path.is_file():
                    raise MoonOutputError(
                        f"{target} runs {rel}, which does not resolve to a readable file under "
                        f"{proj['source_dir']} - A14 would silently scan less. If the script "
                        f"moved, update the task."
                    )
                paths.extend(_uv_script_closure(path, root_resolved, task_bases))
            # R8 (follow-up sweep 2): any other `*.sh` word in the task text that names a repo file,
            # against the repo root and the project directory (`bash ops/nats/check-subjects.sh`).
            for rel in sorted(set(UV_RUN_REF_RE.findall(blob))):
                for base in (root_resolved, root_resolved / proj["source_dir"]):
                    cand = (base / rel).resolve()
                    if cand.is_file() and root_resolved in cand.parents:
                        paths.extend(_uv_script_closure(cand, root_resolved, task_bases))
            for path in dict.fromkeys(paths):
                rel_path = str(path.relative_to(root_resolved))
                reads.add(rel_path)
                text = _read_utf8(path)
                lines = text.split("\n")
                # R10 (follow-up sweep 2): a scanned script that runs version-lockstep --write.
                if rel_path != LOCKSTEP_SCRIPT and runs_lockstep_write(_executable_text(path)):
                    rows.append(
                        f"{target}: {rel_path} can run ci/version-lockstep/run.sh with --write, which runs `uv lock`"
                    )
                for lineno, desc in uv_form_hits(text):
                    line = lines[lineno - 1].strip()
                    if (rel_path, line) in allow:
                        matched.add((rel_path, line))
                        hit_lines.setdefault((rel_path, line), set()).add(lineno)
                        continue
                    rows.append(f"{target}: {rel_path}:{lineno} {desc}")

    # R7 (follow-up sweep 2): uv configuration that makes an approved `uv run` change the
    # environment in place. `uv_configs` is every tracked pyproject.toml and uv.toml
    # (`uv_configs_of`), and `moon_toolchains` is the Moon toolchain file. None skips the part;
    # only the self-test fixtures pass None.
    if uv_configs is not None:
        if REQUIRED_UV_CONFIG not in uv_configs:
            rows.append(f"FLOOR: {REQUIRED_UV_CONFIG} is not in the uv configuration list, so the R7 scan read the wrong set")
        for rel_path in uv_configs:
            reads.add(rel_path)
            rows.extend(uv_config_rows(rel_path, _read_utf8(root_resolved / rel_path)))
    if moon_toolchains is not None:
        if not (root_resolved / moon_toolchains).is_file():
            rows.append(f"FLOOR: {moon_toolchains} is missing, so A14 cannot check the arguments of Moon's `uv sync`")
        else:
            reads.add(moon_toolchains)
            rows.extend(moon_toolchain_rows(moon_toolchains, _read_utf8(root_resolved / moon_toolchains)))

    # R9 (follow-up sweep 2): an EXECUTED waiver covers exactly one physical line. A copy of the
    # waived call elsewhere in the same script is a row.
    for key, linenos in sorted(hit_lines.items()):
        if str(allow.get(key, "")).startswith("EXECUTED") and len(linenos) != 1:
            rows.append(
                f"A14 allowlist entry {key[0]}: {key[1]!r} is EXECUTED but matches {len(linenos)} lines "
                f"({', '.join(map(str, sorted(linenos)))}); an EXECUTED waiver covers one line only"
            )

    # G5: an edit to a file A14 reads must schedule repo:affected-smoke (`smoke_unreached`, shared with A13).
    if scanned:
        unreached = smoke_unreached(projects, reads)
        if unreached is None:
            rows.append(
                f"{AFFECTED_SMOKE_PROJECT}:{AFFECTED_SMOKE_TASK} reported no `inputGlobs`, so A14 cannot check "
                f"that an edit to a script it reads schedules it"
            )
        else:
            for rel_path in unreached:
                rows.append(
                    f"{rel_path} is read by A14 but matches no input of {AFFECTED_SMOKE_PROJECT}:"
                    f"{AFFECTED_SMOKE_TASK}, so an edit to it alone does not schedule the gate that reads it"
                )

    for target in floor:
        pid, _, name = target.partition(":")
        blob = ((projects.get(pid) or {}).get("invocations") or {}).get(name)
        if not blob:
            rows.append(f"FLOOR: {target} is missing or has no command, so A14 asserts nothing for it")
        elif not _floor_isolated(blob):
            rows.append(
                f"FLOOR: {target} does not run `uv run --isolated` with "
                f"`--reinstall-package paigasus-py-bindings`"
            )

    for key in sorted(set(allow) - matched):
        rows.append(f"A14 allowlist entry {key[0]}: {key[1]!r} matches no line of a scanned script (stale)")
    return list(dict.fromkeys(rows))


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
        task_env = {}
        for name, task in (p.get("tasks") or {}).items():
            # SMA-765 (A14): a missing `env` means "no env", not "unknown".
            task_env[name] = dict(task.get("env") or {})
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
            "task_env": task_env,
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
        data = tomllib.loads(_read_utf8(path))
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
        # collect_findings now folds check_dockerfile_locked(root) into a8,
        # check_member_globs(root, crates) into a9, and check_member_literals(root) into a11.
        # ALL THREE raise on an absent file — write a locked Dockerfile and a members list that
        # reaches `crates`' own source dirs, so this arity check stays about arity. The members
        # list is DERIVED from the same `crates` fixture the call passes, so a fixture edit
        # cannot leave this row asserting an arity failure that is really an a9 or a11
        # violation in disguise.
        tmp_rs = Path(tmp) / "rs"
        tmp_rs.mkdir()
        (tmp_rs / "Dockerfile").write_text("RUN cargo build --release --locked -p paigasus-iam\n")
        member_dirs = sorted(
            c["source_dir"].split("/", 1)[1]
            for c in crates.values()
            if c["source_dir"].startswith("rs/")
        )
        (tmp_rs / "Cargo.toml").write_text(
            f"""[workspace]\nmembers = [{", ".join(f'"{d}"' for d in member_dirs)}]\n"""
        )
        # SMA-736: A13 needs git's tracked set. This tmp root is not a repository, so the arity
        # check passes an empty fixed set rather than calling git.
        collected = collect_findings(ok, crates, Path(tmp), frozenset())
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
        if not any(row == "k-ts:test inputs omit rs/crates/libs/kern/src/**/*" for row in rows):
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
        # SMA-658 — the ALLOWLISTED wrapper (`cargo auditable build`, SMA-658's own shape).
        # A wrapped-and-LOCKED invocation must PASS, the same as a plain `cargo build --locked`.
        (rs / "Dockerfile").write_text(
            'RUN cargo auditable build --release --locked -p paigasus-iam\n'
        )
        if check_dockerfile_locked(Path(tmp)):
            failures.append(
                "A8 reported a violation on a locked, wrapped `cargo auditable build` — the "
                "wrapper allowlist did not recognise it as resolving"
            )
        # A wrapped-and-UNLOCKED invocation must still FIRE: recognising the wrapper must not
        # exempt it from the --locked demand.
        (rs / "Dockerfile").write_text(
            'RUN cargo auditable build --release -p paigasus-iam\n'
        )
        if not any("without --locked" in r for r in check_dockerfile_locked(Path(tmp))):
            failures.append(
                "A8 did not fire on an unlocked, wrapped `cargo auditable build` — "
                "recognising the wrapper must not waive --locked"
            )
        # An UNKNOWN wrapper word must still read as no invocation at all and fail the floor
        # CLOSED, not open — CARGO_SUBCOMMAND_WRAPPERS is a narrow allowlist, not a general
        # `\S+`, precisely so a typo'd or unreviewed wrapper cannot silently pass A8.
        (rs / "Dockerfile").write_text(
            'RUN cargo frobnicate build --release --locked -p paigasus-iam\n'
        )
        rows = check_dockerfile_locked(Path(tmp))
        if not any("A8 examines rs/Dockerfile" in r for r in rows):
            failures.append(
                "A8 treated `cargo frobnicate build` as a resolved invocation — an "
                "unallowlisted wrapper word must still read as no cargo invocation at all"
            )
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
                f"""[workspace]\nmembers = [{", ".join(f'"{e}"' for e in entries)}]\n"""
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

    # A11 (SMA-663): no `members` entry may carry a glob character. A glob can match a
    # dot-directory, and `napi build` stages its output in `.<crate>.napi-stage-<random>` beside
    # the crate, with no Cargo.toml in it. A concurrent `cargo metadata` then exits 101.
    with tempfile.TemporaryDirectory() as tmp:
        root11 = Path(tmp)
        rs11 = root11 / "rs"
        rs11.mkdir()

        def write_members11(entries):
            (rs11 / "Cargo.toml").write_text(
                f"""[workspace]\nmembers = [{", ".join(f'"{e}"' for e in entries)}]\n"""
            )

        # a: the SMA-663 shape itself.
        write_members11(["crates/bindings/*"])
        rows = check_member_literals(root11)
        if len(rows) != 1 or "crates/bindings/*" not in rows[0]:
            failures.append(f"A11 did not fire exactly once on `crates/bindings/*`: {rows}")

        # b: the fixed shape — every entry literal.
        write_members11(["crates/bindings/node", "crates/libs/kernel", "crates/services/iam"])
        if check_member_literals(root11):
            failures.append("A11 fired on a members list with only literal entries")

        # c: mixed — one literal, one `*`, one `[` class. Exactly the two globs fire.
        write_members11(["crates/libs/kernel", "crates/services/*", "crates/x/[ab]"])
        rows = check_member_literals(root11)
        if len(rows) != 2 or not any("crates/services/*" in r for r in rows) \
                or not any("crates/x/[ab]" in r for r in rows):
            failures.append(f"A11 did not fire exactly on the `*` and `[` entries: {rows}")

        # d: `?` is a glob character too.
        write_members11(["crates/libs/paigasus-?"])
        if len(check_member_literals(root11)) != 1:
            failures.append("A11 did not fire on a `?` members entry")

        # Infra: the same two shapes as A9, through the shared reader.
        (rs11 / "Cargo.toml").write_text("[workspace]\nresolver = \"3\"\n")
        try:
            check_member_literals(root11)
            failures.append("A11 did not raise infra on a workspace with no `members` key")
        except MoonOutputError:
            pass
        (rs11 / "Cargo.toml").unlink()
        try:
            check_member_literals(root11)
            failures.append("A11 did not raise infra on a missing rs/Cargo.toml")
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
            "invocations": {
                "test": "touch a b && uv run --isolated --reinstall-package paigasus-py-bindings pytest tests"
            },
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

    # A12 (SMA-536). Two halves, like A7-h: a package.json tree written to a tmp root, and moon's
    # resolved view of the `tsc` tasks. The package names are the real ones, so the shapes match
    # what REQUIRED_TS_CLOSURE names.
    if not REQUIRED_TSC_TASKS:
        failures.append("REQUIRED_TSC_TASKS is empty — A12's task floor would assert nothing")
    if not REQUIRED_TS_CLOSURE:
        failures.append("REQUIRED_TS_CLOSURE is empty — A12's closure floor would assert nothing")

    # The tsc token, exercised directly (A10's `_var_sensitive` lesson): through the check, a blob
    # the regex rejects is simply not derived, so a missing row proves nothing about the regex.
    for blob, want in (
        ("pnpm exec tsc -p tsconfig.json --noEmit", True),
        ("tsc --noEmit", True),
        ("touch a && pnpm exec tsc -p tsconfig.json", True),
        ("(tsc -p x)", True),
        ("pnpm exec tsc-alias -p x", False),
        ("pnpm exec napi build --dts index.fresh.d.ts", False),
        ("pnpm exec vitest run", False),
        ("node scripts/tscheck.mjs", False),
    ):
        if bool(TSC_TOKEN_RE.search(blob)) is not want:
            failures.append(
                f"TSC_TOKEN_RE on {blob!r} is {not want} — the tsc token is wrong in the "
                f"{'false-negative' if want else 'false-positive'} direction"
            )

    a12_tree = {
        "ts/packages/kernel": {
            "name": "@paigasus/kernel",
            "dependencies": {"@paigasus/node-bindings": "file:../../../rs/crates/bindings/nb"},
            "exports": {".": "./src/wasm.ts", "./napi": {"types": "./src/index.ts", "default": None}},
        },
        "ts/packages/core": {
            "name": "@paigasus/console-core",
            "dependencies": {"@paigasus/kernel": "workspace:*"},
            "devDependencies": {"@paigasus/proto": "workspace:*"},
            "exports": {
                ".": "./src/index.ts",
                "./testing": "./testing/index.ts",
                "./fakes": {
                    "import": {"types": "./fakes/index.d.ts", "default": "./fakes/index.js"},
                    "require": None,
                },
            },
        },
        "ts/packages/proto": {"name": "@paigasus/proto", "exports": {".": "./src/index.ts"}},
        "ts/packages/nc": {
            "name": "@paigasus/next-config",
            "exports": {".": "./src/index.ts", "./tsconfig-app": "./tsconfig.app.json"},
        },
        "ts/apps/app": {
            "name": "@paigasus/app",
            "dependencies": {"@paigasus/console-core": "workspace:*", "@paigasus/next-config": "workspace:*"},
        },
        "rs/crates/bindings/nb": {"name": "@paigasus/node-bindings", "files": ["index.js", "index.d.ts"]},
    }
    a12_pre = "node ../../../ts/scripts/check-installed-bindings.mjs && pnpm exec tsc -p tsconfig.json --noEmit"
    a12_tsc = "pnpm exec tsc -p tsconfig.json --noEmit"
    a12_nb = ["rs/crates/bindings/nb/index.d.ts", "rs/crates/bindings/nb/package.json", PREFLIGHT_SCRIPT]
    a12_core_files = [TS_BASE_TSCONFIG, "ts/packages/kernel/package.json", "ts/packages/proto/package.json", *a12_nb]
    a12_core_globs = ["ts/packages/kernel/src/**/*", "ts/packages/proto/src/**/*"]

    def _a12_ts(source_dir, tasks, language="typescript"):
        # tasks: {name: (invocation, deps, inputFiles, inputGlobs)}
        return {
            "source_dir": source_dir, "deps": {}, "language": language,
            "tasks": {n: list(t[1]) for n, t in tasks.items()},
            "task_inputs": {n: sorted(set(t[2])) for n, t in tasks.items()},
            "task_input_globs": {n: sorted(set(t[3])) for n, t in tasks.items()},
            "invocations": {n: t[0] for n, t in tasks.items()},
        }

    a12 = {
        "k-ts": _a12_ts("ts/packages/kernel", {
            # Script form: moon reports `command` as the script's first word, so the joined blob
            # repeats it.
            "typecheck": ("node " + a12_pre, [], [TS_BASE_TSCONFIG, *a12_nb], []),
            # Not a tsc task. Its inputs are empty, so an over-matching derivation reds the clean row.
            "test": ("pnpm exec vitest run", [], [], []),
        }),
        "c-ts": _a12_ts("ts/packages/core", {
            "build": (a12_pre, [PROTO_GENERATE_DEP], a12_core_files, a12_core_globs),
            "typecheck": (a12_pre, [PROTO_GENERATE_DEP], a12_core_files, a12_core_globs),
        }),
        "p-ts": _a12_ts("ts/packages/proto", {
            "typecheck": (a12_tsc, [PROTO_GENERATE_DEP], [TS_BASE_TSCONFIG], []),
        }),
        "n-ts": _a12_ts("ts/packages/nc", {"typecheck": (a12_tsc, [], [TS_BASE_TSCONFIG], [])}),
        "app-ts": _a12_ts("ts/apps/app", {
            "typecheck": (
                a12_pre, [PROTO_GENERATE_DEP],
                [*a12_core_files, "ts/packages/core/package.json", "ts/packages/nc/package.json",
                 "ts/packages/nc/tsconfig.app.json"],
                [*a12_core_globs, "ts/packages/core/src/**/*", "ts/packages/core/testing/**/*",
                 "ts/packages/core/fakes/**/*", "ts/packages/nc/src/**/*"],
            ),
        }),
        # A tsc invocation in a NON-typescript project, under-declared on purpose. A12 must not
        # examine it. A clean project here would make the row vacuous (A7-c's lesson).
        "x-py": _a12_ts("py/x", {"typecheck": (a12_tsc, [], [], [])}, language="python"),
    }
    a12_floor = ("k-ts:typecheck", "c-ts:typecheck", "app-ts:typecheck")
    a12_closure_floor = {
        "@paigasus/kernel": {"@paigasus/node-bindings"},
        "@paigasus/console-core": {"@paigasus/kernel"},
    }

    def _a12_copy(obj):
        return json.loads(json.dumps(obj))

    def _a12_run(check_fn, fixture, tree, **kw):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            for rel, data in tree.items():
                (base / rel).mkdir(parents=True, exist_ok=True)
                if data is not None:
                    text = data if isinstance(data, str) else json.dumps(data)
                    (base / rel / "package.json").write_text(text)
            return check_fn(fixture, base, **kw)

    def _a12a(fixture=None, tree=None, **kw):
        kw.setdefault("floor", a12_floor)
        kw.setdefault("closure_floor", a12_closure_floor)
        return _a12_run(
            check_ts_tsc_inputs, a12 if fixture is None else fixture,
            a12_tree if tree is None else tree, **kw,
        )

    rows = _a12a()
    if rows != []:
        failures.append(f"A12a reported violations on a complete fixture: {rows}")

    # A12a-a: an upstream package's src glob, read PER TASK. `build` keeps the glob, so a union
    # across the project's tasks would hide the shortfall (A7-g's lesson).
    broken = _a12_copy(a12)
    broken["c-ts"]["task_input_globs"]["typecheck"] = ["ts/packages/proto/src/**/*"]
    rows = _a12a(broken)
    if "c-ts:typecheck inputs omit ts/packages/kernel/src/**/*" not in rows:
        failures.append("A12a did not fire on a tsc task missing an upstream package's src glob")
    if any(row.startswith("c-ts:build ") for row in rows):
        failures.append("A12a blamed `build` for a shortfall that lives on `typecheck`")

    # A12a-b: the binding half — a `.d.ts` from the binding's `files` list.
    broken = _a12_copy(a12)
    broken["k-ts"]["task_inputs"]["typecheck"].remove("rs/crates/bindings/nb/index.d.ts")
    if "k-ts:typecheck inputs omit rs/crates/bindings/nb/index.d.ts" not in _a12a(broken):
        failures.append("A12a did not demand a binding's .d.ts from its `files` list")

    # A12a-c: the base tsconfig, demanded of every tsc task.
    broken = _a12_copy(a12)
    broken["p-ts"]["task_inputs"]["typecheck"] = []
    if f"p-ts:typecheck inputs omit {TS_BASE_TSCONFIG}" not in _a12a(broken):
        failures.append("A12a did not demand ts/tsconfig.base.json")

    # A12a-d: the preflight script, demanded where the closure holds a binding. p-ts and n-ts
    # declare no preflight input, so the clean row above proves it is not demanded elsewhere.
    broken = _a12_copy(a12)
    broken["k-ts"]["task_inputs"]["typecheck"].remove(PREFLIGHT_SCRIPT)
    if f"k-ts:typecheck inputs omit {PREFLIGHT_SCRIPT}" not in _a12a(broken):
        failures.append("A12a did not demand the preflight script of a task with a binding")

    # A12a-e: an `exports` target in a subdirectory is satisfied by `<dir>/**/*`.
    broken = _a12_copy(a12)
    broken["app-ts"]["task_input_globs"]["typecheck"].remove("ts/packages/core/testing/**/*")
    if "app-ts:typecheck inputs omit ts/packages/core/testing/**/*" not in _a12a(broken):
        failures.append("A12a did not demand the directory glob of a subdirectory `exports` target")

    # A12a-f: a condition object is walked to its string leaves (two levels deep, with a null).
    broken = _a12_copy(a12)
    broken["app-ts"]["task_input_globs"]["typecheck"].remove("ts/packages/core/fakes/**/*")
    if "app-ts:typecheck inputs omit ts/packages/core/fakes/**/*" not in _a12a(broken):
        failures.append("A12a did not walk a conditional `exports` object")

    # A12a-g: an `exports` target at the package root is demanded as the file itself.
    broken = _a12_copy(a12)
    broken["app-ts"]["task_inputs"]["typecheck"].remove("ts/packages/nc/tsconfig.app.json")
    if "app-ts:typecheck inputs omit ts/packages/nc/tsconfig.app.json" not in _a12a(broken):
        failures.append("A12a did not demand a root-level `exports` target")

    # A12a-h: an `exports` shape A12 cannot read reds, and so does a target without `./`.
    tree = _a12_copy(a12_tree)
    tree["ts/packages/core"]["exports"]["./bad"] = ["./bad/a.js"]
    if not any("ts/packages/core/package.json: `exports` holds a list" in r for r in _a12a(tree=tree)):
        failures.append("A12a accepted an `exports` shape it cannot read")
    tree = _a12_copy(a12_tree)
    tree["ts/packages/core"]["exports"]["./raw"] = "raw/index.ts"
    if not any("does not start with `./`" in r for r in _a12a(tree=tree)):
        failures.append("A12a accepted an `exports` target that does not start with ./")

    # A12a-i: a missing DIRECT contracts:generate, for a closure reader and for proto itself. n-ts
    # has no proto in its closure and declares no deps, so the clean row proves no over-demand.
    broken = _a12_copy(a12)
    broken["app-ts"]["tasks"]["typecheck"] = []
    broken["p-ts"]["tasks"]["typecheck"] = []
    rows = _a12a(broken)
    for target in ("app-ts:typecheck", "p-ts:typecheck"):
        if not any(r.startswith(f"{target} deps omit {PROTO_GENERATE_DEP}") for r in rows):
            failures.append(f"A12a did not demand {PROTO_GENERATE_DEP} of {target}")

    # A12a-j: the task floor. No task matches `tsc`, so the derived set is empty and every
    # per-task row goes quiet by itself. The row must name THIS branch's message.
    broken = _a12_copy(a12)
    for proj in broken.values():
        proj["invocations"] = dict.fromkeys(proj["invocations"], "pnpm exec vitest run")
    if not any(
        r.startswith("FLOOR:") and "k-ts:typecheck is not matched by the `tsc` token" in r
        for r in _a12a(broken)
    ):
        failures.append("A12a's task floor did not fire when no task matches `tsc`")

    # A12a-k: the closure floor. The kernel loses its binding edge, and a floor package is absent.
    tree = _a12_copy(a12_tree)
    tree["ts/packages/kernel"]["dependencies"] = {}
    if not any(
        r.startswith("FLOOR:") and "closure of @paigasus/kernel no longer derives @paigasus/node-bindings" in r
        for r in _a12a(tree=tree)
    ):
        failures.append("A12a's closure floor did not fire on a broken package.json walk")
    if not any(
        r.startswith("FLOOR:") and "@paigasus/ghost is not a package" in r
        for r in _a12a(closure_floor={"@paigasus/ghost": {"@paigasus/kernel"}})
    ):
        failures.append("A12a's closure floor did not fire on a package that does not exist")

    # A12a-l to A12a-o: the fail-closed walk. Each case is a row, never a skip.
    tree = _a12_copy(a12_tree)
    tree["ts/packages/core"]["devDependencies"]["@paigasus/ghost"] = "workspace:*"
    if not any("@paigasus/ghost maps to no package directory" in r for r in _a12a(tree=tree)):
        failures.append("A12a skipped a `workspace:` name with no package")
    tree = _a12_copy(a12_tree)
    tree["ts/packages/core"]["dependencies"]["@paigasus/linked"] = "link:../linked"
    if not any("@paigasus/linked uses a `link:` specifier" in r for r in _a12a(tree=tree)):
        failures.append("A12a skipped a `link:` specifier")
    tree = _a12_copy(a12_tree)
    tree["rs/crates/bindings/nb"] = None
    if not any("rs/crates/bindings/nb/package.json is missing" in r for r in _a12a(tree=tree)):
        failures.append("A12a skipped a missing package.json")
    tree = _a12_copy(a12_tree)
    tree["ts/packages/proto"] = "{"
    if not any("ts/packages/proto/package.json is not valid JSON" in r for r in _a12a(tree=tree)):
        failures.append("A12a skipped an unparseable package.json")
    tree = _a12_copy(a12_tree)
    tree["ts/packages/kernel"]["dependencies"]["@paigasus/node-bindings"] = "file:../../../rs/crates/bindings/ghost"
    if not any("which does not exist" in r for r in _a12a(tree=tree)):
        failures.append("A12a skipped a `file:` path that does not exist")

    # A12a-p: a task with no input bucket is a violation, never a skip.
    broken = _a12_copy(a12)
    del broken["c-ts"]["task_input_globs"]["typecheck"]
    if not any(
        r.startswith("c-ts:typecheck reported no `inputFiles`/`inputGlobs`") and "moon's output shape changed" in r
        for r in _a12a(broken)
    ):
        failures.append("A12a skipped a task that reported no input bucket")

    # A12a-q: a workspace cycle back to the task's own package. The own package is excluded at
    # every depth, so core is not asked to key on itself, and the walk ends.
    tree = _a12_copy(a12_tree)
    tree["ts/packages/proto"]["devDependencies"] = {"@paigasus/console-core": "workspace:*"}
    if any(r.startswith("c-ts:") for r in _a12a(tree=tree)):
        failures.append("A12a demanded a package's own files through a workspace cycle")

    # A12a-r: a None invocation is moon telling us nothing. That is infra, exactly as in A5.
    broken = _a12_copy(a12)
    broken["k-ts"]["invocations"]["typecheck"] = None
    try:
        derive_tsc_tasks(broken)
    except MoonOutputError:
        pass
    else:
        failures.append("derive_tsc_tasks accepted a task with no command, script or args")

    def _a12b(fixture=None, tree=None, **kw):
        kw.setdefault("floor", a12_floor)
        return _a12_run(
            check_ts_tsc_preflight, a12 if fixture is None else fixture,
            a12_tree if tree is None else tree, **kw,
        )

    rows = _a12b()
    if rows != []:
        failures.append(f"A12b reported violations on a complete fixture: {rows}")

    # The order verdict, exercised directly. `None` means no violation; a string must appear in the
    # verdict. Row 2 is moon's joined form of a script task (the first word repeats).
    for blob, want in (
        ("node ../../../ts/scripts/check-installed-bindings.mjs && pnpm exec tsc -p tsconfig.json --noEmit", None),
        ("node node ../../../ts/scripts/check-installed-bindings.mjs && pnpm exec tsc -p tsconfig.json --noEmit", None),
        ("touch a && pnpm exec napi build && node ../../../ts/scripts/check-installed-bindings.mjs && pnpm exec tsc -p x", None),
        ("node ts/scripts/check-installed-bindings.mjs && tsc", None),
        ("pnpm exec tsc -p x", "does not run"),
        ("node ../../../ts/scripts/check-installed-bindings.mjsx && pnpm exec tsc -p x", "does not run"),
        ("echo ts/scripts/check-installed-bindings.mjs && pnpm exec tsc -p x", "does not run"),
        ("pnpm exec tsc -p x && node ts/scripts/check-installed-bindings.mjs", "after `tsc`"),
        ("node ts/scripts/check-installed-bindings.mjs; pnpm exec tsc -p x", "with `;`"),
        ("node ts/scripts/check-installed-bindings.mjs || pnpm exec tsc -p x", "with `||`"),
        ("node ts/scripts/check-installed-bindings.mjs | pnpm exec tsc -p x", "with `|`"),
        ("node ts/scripts/check-installed-bindings.mjs & pnpm exec tsc -p x", "with `&`"),
        ("node ts/scripts/check-installed-bindings.mjs\npnpm exec tsc -p x", "with a newline"),
    ):
        got = preflight_order_verdict(blob)
        if (want is None and got is not None) or (want is not None and (got is None or want not in got)):
            failures.append(
                f"preflight_order_verdict({blob!r}) is {got!r}, expected "
                f"{'no violation' if want is None else repr(want)}"
            )

    # A12b-a: a binding task with no preflight.
    broken = _a12_copy(a12)
    broken["k-ts"]["invocations"]["typecheck"] = a12_tsc
    if not any(r.startswith("k-ts:typecheck does not run") for r in _a12b(broken)):
        failures.append("A12b did not fire on a binding task with no preflight")

    # A12b-b: a preflight after `tsc`, read PER TASK (c-ts:build keeps the right order).
    broken = _a12_copy(a12)
    broken["c-ts"]["invocations"]["typecheck"] = a12_tsc + " && node ../../../ts/scripts/check-installed-bindings.mjs"
    rows = _a12b(broken)
    if not any(r.startswith("c-ts:typecheck runs") and "after `tsc`" in r for r in rows):
        failures.append("A12b did not fire on a preflight after `tsc`")
    if any(r.startswith("c-ts:build ") for r in rows):
        failures.append("A12b blamed `build` for a script that lives on `typecheck`")

    # A12b-c: a preflight joined with `;`.
    broken = _a12_copy(a12)
    broken["app-ts"]["invocations"]["typecheck"] = a12_pre.replace(" && ", "; ", 1)
    if not any(r.startswith("app-ts:typecheck joins") and "with `;`" in r for r in _a12b(broken)):
        failures.append("A12b did not fire on a preflight joined with `;`")

    # A12b-d: the floor. A floor task with no binding in its closure is not examined, and so is
    # every task when the kernel loses its binding edge. Both must red, with THIS branch's prefix.
    if not any(r.startswith("FLOOR:") and "p-ts:typecheck" in r for r in _a12b(floor=("p-ts:typecheck",))):
        failures.append("A12b's floor did not fire on a floor task it does not examine")
    tree = _a12_copy(a12_tree)
    tree["ts/packages/kernel"]["dependencies"] = {}
    if not any(r.startswith("FLOOR:") and "k-ts:typecheck" in r for r in _a12b(tree=tree)):
        failures.append("A12b's floor did not fire when a floor task's closure lost its binding")

    # SMA-736 T12 — the A12/A13 shared helper, called directly. A12a's rows above already pass
    # through it; this row pins the helper's own contract, so a change to it reds here first.
    rows = []
    got = workspace_package_inputs("ts/packages/core", {
        "exports": {
            ".": "./src/index.ts",
            "./testing": "./testing/index.ts",
            "./preset": {"import": "./preset.json", "require": None},
        },
    }, rows)
    want = {
        "ts/packages/core/package.json", "ts/packages/core/src/**/*",
        "ts/packages/core/testing/**/*", "ts/packages/core/preset.json",
    }
    if got != want or rows:
        failures.append(f"workspace_package_inputs gave {sorted(got)} and rows {rows}, expected {sorted(want)} and no rows")
    rows = []
    workspace_package_inputs("ts/packages/core", {"exports": {".": "src/index.ts"}}, rows)
    if rows != ["ts/packages/core/package.json: the `exports` target 'src/index.ts' does not start with `./`"]:
        failures.append(f"workspace_package_inputs accepted an `exports` target without `./`: {rows}")
    # A13 (SMA-736) — the config parser, called directly (spec T5). Each row is (a13_label, config text,
    # expected aliases or None to skip the check, expected tsconfig_off or None, the one expected row
    # or None for "no rows at all"). The comment, string and template rows are the SMA-639 defect
    # class: a scanner that opens a string inside a comment, or a comment inside a string, loses or
    # invents an alias.
    a13_cfg = "ts/packages/k/vitest.config.ts"
    a13_ok_alias = [("a", "ts/packages/k/a")]
    for a13_label, a13_text, a13_want_aliases, a13_want_off, a13_want_row in (
        ("an `alias` array", "export default { resolve: { alias: [{ find: 'a', replacement: './a' }] } };\n",
         None, None, f"{a13_cfg}: an `alias` value is not an object literal, so A13 cannot read it"),
        ("a shorthand `alias`", "const alias = { a: './a' };\nexport default { resolve: { alias } };\n",
         None, None, f"{a13_cfg}: `alias` is used without a `:` value (a shorthand or a variable), which A13 cannot read"),
        ("a quoted 'alias' key", "export default { resolve: { 'alias': { '@paigasus/x': '../x/src/index.ts' } } };\n",
         [("@paigasus/x", "ts/packages/x/src/index.ts")], False, None),
        ("a call as a @paigasus/ value", "export default { resolve: { alias: { '@paigasus/x': path.resolve('x') } } };\n",
         None, None, f"{a13_cfg}: the alias @paigasus/x has a value A13 cannot resolve"),
        ("a package name as a @paigasus/ value", "export default { resolve: { alias: { '@paigasus/x': 'other-pkg' } } };\n",
         None, None, f"{a13_cfg}: the alias @paigasus/x has a value A13 cannot resolve"),
        ("an unbound identifier value", "export default { resolve: { alias: { 'server-only': stub } } };\n",
         None, None, f"{a13_cfg}: the alias server-only has a value A13 cannot resolve"),
        ("a package name as a plain value", "export default { resolve: { alias: { react: 'preact/compat' } } };\n",
         [], False, None),
        ("a spread entry", "export default { resolve: { alias: { ...base } } };\n",
         None, None, f"{a13_cfg}: an `alias` object holds a spread entry, which A13 cannot read"),
        ("a computed key", "export default { resolve: { alias: { [k]: './a' } } };\n",
         None, None, f"{a13_cfg}: an `alias` object holds a computed key, which A13 cannot read"),
        ("a trailing comma", "export default { resolve: { alias: { a: './a', } } };\n", a13_ok_alias, False, None),
        ("a `//` comment", "// alias: { '@paigasus/x': stub }\nexport default {};\n", [], False, None),
        ("a `/* */` comment", "/* alias: { '@paigasus/x': stub } */\nexport default {};\n", [], False, None),
        ("`//` inside a string", "const u = 'http://x'; export default { resolve: { alias: { a: './a' } } };\n",
         a13_ok_alias, False, None),
        ("quotes inside comments", "// it's \"a\" `b`\n/* it's \"a\" `b` */\nexport default { resolve: { alias: { a: './a' } } };\n",
         a13_ok_alias, False, None),
        ("a template literal",
         "const m = `x \\` ${ {a: 1}.a } alias: { '@paigasus/y': 1 }`;\nexport default { resolve: { alias: { a: './a' } } };\n",
         a13_ok_alias, False, None),
        ("a regex literal", "export default { test: { server: { deps: { external: [/\\.node$/] } } }, resolve: { alias: { a: './a' } } };\n",
         a13_ok_alias, False, None),
        ("a relative import", "import base from './base.config';\nexport default base;\n",
         None, None, f"{a13_cfg}: imports './base.config'; A13 cannot read a config that is split across files"),
        ("mergeConfig", "import { mergeConfig } from 'vitest/config';\nexport default {};\n",
         None, None, f"{a13_cfg}: uses `mergeConfig`; A13 cannot read a config that is split across files"),
        ("a string `projects` entry", "export default { test: { projects: ['packages/*'] } };\n",
         None, None, f"{a13_cfg}: `projects` holds a string entry; vitest loads it as another config or a glob, which A13 does not follow"),
        ("object `projects` entries", "export default { test: { projects: [{ test: { include: ['a.test.ts'] } }] } };\n", [], False, None),
        ("`tsconfig: false`", "const oxc = { tsconfig: false } as const;\nexport default { oxc };\n", [], True, None),
        ("`tsconfig: false` in a comment", "// tsconfig: false\nexport default {};\n", [], False, None),
        ("a fileURLToPath binding",
         "const d = fileURLToPath(new URL('../../../rs/crates/bindings/nb/index.js', import.meta.url));\n"
         "export default { resolve: { alias: { '@paigasus/node-bindings': d } } };\n",
         [("@paigasus/node-bindings", "rs/crates/bindings/nb/index.js")], False, None),
        ("a regex literal with `//`", "export default { test: { exclude: [/^https?:\\/\\//] }, resolve: { alias: { '@paigasus/x': stub } } };\nconst stub = './x';\n",
         [("@paigasus/x", "ts/packages/k/x")], False, None),
        ("a regex literal with `/*`", "const r = /a\\/*b/;\nconst stub = './x';\nexport default { resolve: { alias: { '@paigasus/x': stub } } };\n// */\n",
         [("@paigasus/x", "ts/packages/k/x")], False, None),
        ("a division", "const d = a / b / c; export default { resolve: { alias: { a: './a' } } };\n// it's\n",
         a13_ok_alias, False, None),
        ("a regex class with `/`", "export default { test: { exclude: [/[/]/] }, resolve: { alias: { a: './a' } } };\n",
         a13_ok_alias, False, None),
        ("an unterminated regex", "export default { test: { exclude: [/abc] } };\n",
         None, None, f"{a13_cfg}: an unterminated regex literal at line 1, so A13 cannot scan it"),
        ("a regex after `=>`", "export default { test: { filter: (u) => /^https?:\\/\\//.test(u) }, resolve: { alias: { a: './a' } } };\n",
         a13_ok_alias, False, None),
        ("a regex after `typeof`", "const t = typeof /a\\/\\//; export default { resolve: { alias: { a: './a' } } };\n",
         a13_ok_alias, False, None),
        ("a division after `++`", "export default { n: i++ / 2, resolve: { alias: { a: k } }, m: 3 / 4 };\nconst k = './a';\n",
         a13_ok_alias, False, None),
        ("a division after `--`", "export default { n: i-- / 2, resolve: { alias: { a: k } }, m: 3 / 4 };\nconst k = './a';\n",
         a13_ok_alias, False, None),
        ("a regex after `+`", "const t = 'x' + /^https?:\\/\\//.source; export default { resolve: { alias: { a: './a' } } };\n",
         a13_ok_alias, False, None),
        ("a comment marker after a backslash", "const t = a \\// b;\nexport default { resolve: { alias: { a: './a' } } };\n",
         None, None, f"{a13_cfg}: a comment marker after a backslash at line 1; A13 cannot scan it"),
        ("a backslash before a newline in a regex", "const r = /a\\\n/;\nexport default { resolve: { alias: { a: './a' } } };\n",
         None, None, f"{a13_cfg}: an unterminated regex literal at line 1, so A13 cannot scan it"),
        ("a computed ['alias'] key", "export default { resolve: { ['alias']: { a: './a' } } };\n",
         None, None, f"{a13_cfg}: a computed key ['alias'] holds an alias object, which A13 cannot read"),
        ("a relative require", "const base = require('./base');\nexport default base;\n",
         None, None, f"{a13_cfg}: imports './base'; A13 cannot read a config that is split across files"),
        ("an unclosed string", "const s = 'abc\nexport default {};\n",
         None, None, f"{a13_cfg}: a ' string is not closed on its line, so A13 cannot read the file"),
    ):
        a13_rows = []
        a13_aliases, a13_off = vitest_config_facts(a13_text, a13_cfg, a13_rows)
        if a13_want_row is None and a13_rows:
            failures.append(f"A13 parser, {a13_label}: unexpected rows {a13_rows}")
        if a13_want_row is not None and a13_want_row not in a13_rows:
            failures.append(f"A13 parser, {a13_label}: expected the row {a13_want_row!r}, got {a13_rows}")
        if a13_want_aliases is not None and a13_aliases != a13_want_aliases:
            failures.append(f"A13 parser, {a13_label}: aliases {a13_aliases}, expected {a13_want_aliases}")
        if a13_want_off is not None and a13_off is not a13_want_off:
            failures.append(f"A13 parser, {a13_label}: tsconfig_off is {a13_off}, expected {a13_want_off}")
    # A13 (SMA-736) — the vitest token, exercised directly (A10's `_var_sensitive` lesson).
    for a13_blob, a13_want in (
        ("pnpm exec vitest run", True),
        ("vitest run --passWithNoTests", True),
        ("pnpm exec vitest", True),
        ("set set -euo pipefail\npnpm exec vitest run --config vitest.containers.config.ts\n", True),
        ("touch a && ( cd b && wasm-pack build ) && pnpm exec vitest run", True),
        ("node vitest.config.ts", False),
        ("pnpm exec vitest-environment-x", False),
        ("pnpm add -D @vitest/coverage-v8", False),
        ("pnpm exec tsc -p tsconfig.json --noEmit", False),
    ):
        if bool(VITEST_TOKEN_RE.search(a13_blob)) is not a13_want:
            failures.append(
                f"VITEST_TOKEN_RE on {a13_blob!r} is {not a13_want} — the vitest token is wrong in the "
                f"{'false-negative' if a13_want else 'false-positive'} direction"
            )

    # Moon reports a script-form task with `command` set to the script's first word, so the joined
    # blob repeats it (see PREFLIGHT_RE). The rows with `set set` model that.
    # A13 — config lookup from the invocation (spec T6). Each row is (invocation, source dir,
    # expected configs, the one expected row or None for "no rows at all").
    a13_k = "ts/packages/k"
    a13_rel_row = (
        "t:test runs vitest with the config path $CFG, which A13 cannot resolve (it holds `$`, a glob "
        "character or a quote, or it leaves the repository)"
    )
    with tempfile.TemporaryDirectory() as a13_tmp:
        a13_root = Path(a13_tmp)
        for a13_rel in (
            "ts/packages/k/vitest.config.ts",
            "ts/packages/k/vitest.e2e.config.ts",
            "ts/packages/m/vitest.config.mts",
            "ts/packages/v/vite.config.ts",
            "ts/packages/v/vitest.config.js",
        ):
            (a13_root / a13_rel).parent.mkdir(parents=True, exist_ok=True)
            (a13_root / a13_rel).write_text("export default {};\n")
        (a13_root / "ts/packages/n").mkdir(parents=True)
        a13_e2e = ["ts/packages/k/vitest.e2e.config.ts"]
        for a13_blob, a13_own, a13_configs, a13_row in (
            ("pnpm exec vitest run --config vitest.e2e.config.ts", a13_k, a13_e2e, None),
            ("pnpm exec vitest run --config=vitest.e2e.config.ts", a13_k, a13_e2e, None),
            ("pnpm exec vitest run -c vitest.e2e.config.ts", a13_k, a13_e2e, None),
            ("pnpm exec vitest run --config ./vitest.e2e.config.ts", a13_k, a13_e2e, None),
            ("set set -euo pipefail\npnpm exec vitest run\npnpm exec vitest run --config vitest.e2e.config.ts\n", a13_k,
             ["ts/packages/k/vitest.config.ts", *a13_e2e], None),
            # A newline after a bare `vitest` ends its span: the next call's flags are not its flags.
            ("pnpm exec vitest\npnpm exec vitest run --config vitest.e2e.config.ts\n", a13_k,
             ["ts/packages/k/vitest.config.ts", *a13_e2e], None),
            ("pnpm exec vitest run --config vitest.ghost.config.ts", a13_k, [],
             "t:test runs vitest with ts/packages/k/vitest.ghost.config.ts, which does not exist"),
            ("pnpm exec vitest run --passWithNoTests", "ts/packages/n", [], None),
            ("pnpm exec vitest run", "ts/packages/m", ["ts/packages/m/vitest.config.mts"], None),
            ("pnpm exec vitest run", "ts/packages/v", ["ts/packages/v/vitest.config.js"], None),
            ("pnpm exec vitest run --root sub", a13_k, [],
             "t:test runs vitest with --root, so A13 cannot tell which config vitest reads"),
            ("pnpm exec vitest run -r sub", a13_k, [],
             "t:test runs vitest with -r, so A13 cannot tell which config vitest reads"),
            ("cd sub && pnpm exec vitest run", a13_k, [],
             "t:test changes directory with `cd` before it runs vitest, so A13 cannot tell which config vitest reads"),
            ("touch a && ( cd ../x && wasm-pack build . --out-dir o ) && pnpm exec vitest run", a13_k,
             ["ts/packages/k/vitest.config.ts"], None),
            ("pnpm exec vitest run -c=vitest.e2e.config.ts", a13_k, a13_e2e, None),
            ("pnpm exec vitest run -r=sub", a13_k, [],
             "t:test runs vitest with -r, so A13 cannot tell which config vitest reads"),
            ("pnpm -C sub exec vitest run", a13_k, [],
             "t:test changes directory with `cd` before it runs vitest, so A13 cannot tell which config vitest reads"),
            ("pnpm --dir=sub exec vitest run", a13_k, [],
             "t:test changes directory with `cd` before it runs vitest, so A13 cannot tell which config vitest reads"),
            ("pnpm --dir sub exec vitest run", a13_k, [],
             "t:test changes directory with `cd` before it runs vitest, so A13 cannot tell which config vitest reads"),
            ("pnpm exec vitest run --config $CFG", a13_k, [], a13_rel_row),
            # A `cd` right after `(` is in the same subshell as the vitest call: a row.
            ("(cd sub && pnpm exec vitest run)", a13_k, [],
             "t:test changes directory with `cd` before it runs vitest, so A13 cannot tell which config vitest reads"),
            # A closed group that holds the first vitest call is removed only for the later call.
            ("( cd sub && pnpm exec vitest run ) && pnpm exec vitest run", a13_k, ["ts/packages/k/vitest.config.ts"],
             "t:test changes directory with `cd` before it runs vitest, so A13 cannot tell which config vitest reads"),
        ):
            a13_rows = []
            a13_got = vitest_invocation_configs("t:test", a13_blob, a13_root, a13_own, a13_rows)
            if a13_got != a13_configs or (a13_row is None and a13_rows) or (a13_row is not None and a13_row not in a13_rows):
                failures.append(
                    f"vitest_invocation_configs({a13_blob!r}, {a13_own!r}) gave {a13_got} and rows {a13_rows}, expected "
                    f"{a13_configs} and {'no row' if a13_row is None else repr(a13_row)}"
                )
        # ...and a `--root` on the line after a bare `vitest` makes one row, for the second call only.
        a13_rows = []
        a13_got = vitest_invocation_configs("t:test", "pnpm exec vitest\npnpm exec vitest run --root x\n", a13_root, a13_k, a13_rows)
        if a13_got != ["ts/packages/k/vitest.config.ts"] or len(a13_rows) != 1:
            failures.append(f"vitest_invocation_configs on a bare vitest before a --root line gave {a13_got} and rows {a13_rows}, expected the default config and one row")
    # A13 (SMA-736 §4.5) — the tracked-file plumbing that main() uses. tracked_files raises
    # task_inputs' OWN MoonOutputError, a different class from this file's. It must be in
    # INFRA_ERRORS, or a failed `git ls-files` exits 1 with a traceback instead of rc 2.
    with tempfile.TemporaryDirectory() as tmp:
        try:
            task_inputs.tracked_files(Path(tmp))
        except INFRA_ERRORS:
            pass
        except Exception as exc:
            failures.append(
                f"tracked_files outside a repository raised {type(exc).__name__}, which INFRA_ERRORS does "
                f"not catch, so main() would exit 1 instead of rc 2"
            )
        else:
            failures.append("tracked_files outside a repository returned a set instead of raising")
    # ...and the git call must ignore an inherited GIT_DIR, GIT_INDEX_FILE and GIT_WORK_TREE (a git hook sets them).
    a13_repo_root = Path(__file__).resolve().parents[2]
    a13_saved_env = {k: os.environ.get(k) for k in ("GIT_DIR", "GIT_INDEX_FILE", "GIT_WORK_TREE")}
    os.environ["GIT_DIR"] = "/nonexistent/sma-736-git-dir"
    os.environ["GIT_WORK_TREE"] = "/nonexistent/sma-736-work-tree"
    os.environ["GIT_INDEX_FILE"] = "/nonexistent/sma-736-index"
    try:
        a13_tracked_real = task_inputs.tracked_files(a13_repo_root)
    except Exception as exc:
        a13_tracked_real = None
        failures.append(f"tracked_files used an inherited GIT_DIR or GIT_INDEX_FILE: {exc}")
    finally:
        for k, v in a13_saved_env.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v
    if a13_tracked_real is not None and "ci/affected-graph/cargo_moon_parity.py" not in a13_tracked_real:
        failures.append("tracked_files does not list this file, so it did not read this repository")
    # A13 (SMA-736) — the whole check. Two halves, like A12: a file tree written to a tmp root, and
    # moon's resolved view of the vitest tasks. `a13_tracked` stands in for `git ls-files`; the one
    # path it leaves out, rs/crates/bindings/wb/.out/wb.js, is the kernel's scratch alias target (D3).
    if not REQUIRED_VITEST_TASKS:
        failures.append("REQUIRED_VITEST_TASKS is empty — A13's task floor would assert nothing")
    if not REQUIRED_VITEST_ALIASES:
        failures.append("REQUIRED_VITEST_ALIASES is empty — A13's alias floor would assert nothing")

    a13_files = {
        "ts/tsconfig.base.json": '{"compilerOptions": {"strict": true}}\n',
        "ts/packages/kernel/package.json": json.dumps({
            "name": "@paigasus/kernel",
            "dependencies": {
                "@paigasus/node-bindings": "file:../../../rs/crates/bindings/nb",
                "@paigasus/wasm": "file:../../../rs/crates/bindings/wb",
            },
            "exports": {".": "./src/wasm.ts"},
        }),
        "ts/packages/kernel/src/wasm.ts": "export {};\n",
        # JSONC, as the real kernel and ui tsconfig files are. A plain json.loads fails on it.
        "ts/packages/kernel/tsconfig.json": '// JSONC, as in the real kernel package.\n{"extends": "../../tsconfig.base.json"}\n',
        "ts/packages/kernel/vitest.config.ts": (
            "import { fileURLToPath } from 'node:url';\n"
            "import { defineConfig } from 'vitest/config';\n"
            "const nb = fileURLToPath(new URL('../../../rs/crates/bindings/nb/index.js', import.meta.url));\n"
            "const wb = fileURLToPath(new URL('../../../rs/crates/bindings/wb/.out/wb.js', import.meta.url));\n"
            "export default defineConfig({\n"
            "  test: { projects: [\n"
            "    { resolve: { alias: { '@paigasus/node-bindings': nb } } },\n"
            "    { resolve: { alias: { '@paigasus/wasm': wb } } },\n"
            "  ] },\n"
            "});\n"
        ),
        "rs/crates/bindings/nb/package.json": json.dumps({"name": "@paigasus/node-bindings", "files": ["index.js", "index.d.ts"]}),
        "rs/crates/bindings/nb/index.js": "",
        "rs/crates/bindings/nb/index.d.ts": "",
        # `./wb.js` proves the normalization: the clean fixture declares `rs/crates/bindings/wb/wb.js`.
        "rs/crates/bindings/wb/package.json": json.dumps({"name": "@paigasus/wasm", "files": ["./wb.js", "wb_bg.wasm", "wb.d.ts"]}),
        "rs/crates/bindings/wb/wb.js": "",
        "rs/crates/bindings/wb/wb_bg.wasm": "",
        "rs/crates/bindings/wb/wb.d.ts": "",
        # No tsconfig.json: a missing tsconfig adds nothing.
        "ts/packages/proto/package.json": json.dumps({"name": "@paigasus/proto", "exports": {".": "./src/index.ts"}}),
        "ts/packages/core/package.json": json.dumps({
            "name": "@paigasus/console-core",
            "dependencies": {"@paigasus/kernel": "workspace:*"},
            "devDependencies": {"@paigasus/proto": "workspace:*"},
            "exports": {".": "./src/index.ts", "./testing": "./testing/index.ts"},
        }),
        "ts/packages/core/tsconfig.json": '{"extends": "../../tsconfig.base.json"}\n',
        # One alias inside the own package (adds nothing) and one tracked target outside it (demanded
        # as a literal, although ts/packages/kernel/src/**/* covers it).
        "ts/packages/core/vitest.config.ts": (
            "import { defineConfig } from 'vitest/config';\n"
            "export default defineConfig({\n"
            "  resolve: { alias: { 'server-only': './tests/stub.ts', '@paigasus/kernel': '../kernel/src/wasm.ts' } },\n"
            "});\n"
        ),
        "ts/packages/core/vitest.e2e.config.ts": (
            "import { defineConfig } from 'vitest/config';\n"
            "export default defineConfig({ test: { include: ['tests/e2e/**'] } });\n"
        ),
        "ts/apps/app/package.json": json.dumps({"name": "@paigasus/app", "dependencies": {"@paigasus/console-core": "workspace:*"}}),
        # A package-specifier `extends` is a row, but only when A13 reads the file. The app config
        # sets `tsconfig: false`, so the clean fixture never reads it.
        "ts/apps/app/tsconfig.json": '{"extends": "@paigasus/next-config/tsconfig-app"}\n',
        "ts/apps/app/vitest.config.ts": (
            "import { defineConfig } from 'vitest/config';\n"
            "const oxc = { tsconfig: false } as const;\n"
            "export default defineConfig({ oxc });\n"
        ),
    }
    a13_tracked = frozenset(a13_files)
    a13_lock = TS_LOCKFILE
    a13_nb = ["rs/crates/bindings/nb/package.json", "rs/crates/bindings/nb/index.js"]
    a13_wb = ["rs/crates/bindings/wb/package.json", "rs/crates/bindings/wb/wb.js", "rs/crates/bindings/wb/wb_bg.wasm"]
    a13_core_files = [
        a13_lock, "ts/packages/kernel/package.json", "ts/packages/proto/package.json", *a13_nb, *a13_wb,
        "ts/packages/core/tsconfig.json", "ts/packages/kernel/tsconfig.json", TS_BASE_TSCONFIG,
    ]
    a13_core_globs = ["ts/packages/kernel/src/**/*", "ts/packages/proto/src/**/*"]
    # The real kernel test's shape: an FFI build, a `cd` inside a subshell, then vitest.
    a13_ffi = (
        "touch touch x && pnpm exec napi build --platform && "
        "( cd ../../../rs/crates/bindings/wb && wasm-pack build . --out-dir .out ) && pnpm exec vitest run"
    )
    a13 = {
        "k-ts": _a12_ts("ts/packages/kernel", {
            "test": (a13_ffi, [], [
                a13_lock, "ts/packages/kernel/vitest.config.ts", *a13_nb, *a13_wb,
                "ts/packages/kernel/tsconfig.json", TS_BASE_TSCONFIG,
            ], []),
        }),
        "c-ts": _a12_ts("ts/packages/core", {
            "test": (
                "pnpm exec vitest run --passWithNoTests", [PROTO_GENERATE_DEP],
                [*a13_core_files, "ts/packages/core/vitest.config.ts", "ts/packages/kernel/src/wasm.ts"], a13_core_globs,
            ),
            "test-e2e": (
                "set set -euo pipefail\npnpm exec vitest run --config vitest.e2e.config.ts\n", [PROTO_GENERATE_DEP],
                [*a13_core_files, "ts/packages/core/vitest.e2e.config.ts"], a13_core_globs,
            ),
        }),
        # No config file: vitest runs with defaults (as paigasus-proto-ts:test does).
        "p-ts": _a12_ts("ts/packages/proto", {
            "test": ("pnpm exec vitest run --passWithNoTests", [PROTO_GENERATE_DEP], [a13_lock], []),
        }),
        "app-ts": _a12_ts("ts/apps/app", {
            "test": (
                "pnpm exec vitest run", [PROTO_GENERATE_DEP],
                [a13_lock, "ts/apps/app/vitest.config.ts", "ts/packages/core/package.json", "ts/packages/kernel/package.json",
                 "ts/packages/proto/package.json", *a13_nb, *a13_wb],
                [*a13_core_globs, "ts/packages/core/src/**/*", "ts/packages/core/testing/**/*"],
            ),
        }),
        # A vitest call in a NON-typescript project, under-declared on purpose. A13 must not examine it.
        "x-py": _a12_ts("py/x", {"test": ("pnpm exec vitest run", [], [], [])}, language="python"),
        "repo": _a12_ts(".", {
            "affected-smoke": ("true", [], [], ["ts/packages/*/vitest*.config.*", "ts/apps/*/vitest*.config.*"]),
        }, language="bash"),
    }
    a13_floor = ("k-ts:test", "c-ts:test", "c-ts:test-e2e", "app-ts:test")
    a13_alias_floor = {"k-ts:test": {"@paigasus/node-bindings", "@paigasus/wasm"}}

    def _a13(fixture=None, files=None, tracked=None, **kw):
        # `files` maps a path to its text; None deletes the path from the tree.
        kw.setdefault("floor", a13_floor)
        kw.setdefault("alias_floor", a13_alias_floor)
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            for rel, text in (a13_files if files is None else files).items():
                if text is None:
                    continue
                (base / rel).parent.mkdir(parents=True, exist_ok=True)
                (base / rel).write_text(text)
            return check_ts_vitest_inputs(
                a13 if fixture is None else fixture, base,
                a13_tracked if tracked is None else tracked, **kw,
            )

    # T1. Clean. This also proves: the subshell `cd` in k-ts is not a row; the scratch alias with the
    # matching --out-dir passes (D3); the in-package alias adds nothing; no `.d.ts` is demanded;
    # `./wb.js` is normalized; the app's `tsconfig: false` skips its tsconfig; x-py is ignored.
    rows = _a13()
    if rows != []:
        failures.append(f"A13 reported violations on a complete fixture: {rows}")

    # T2. One mutation per part of `want`, each read PER TASK: c-ts:test-e2e keeps every entry that
    # c-ts:test loses, so a union across the project's tasks would hide the shortfall (A7-g's lesson).
    for pid, task, bucket, entry in (
        ("c-ts", "test", "task_inputs", a13_lock),
        ("c-ts", "test", "task_inputs", "ts/packages/core/vitest.config.ts"),
        ("app-ts", "test", "task_input_globs", "ts/packages/kernel/src/**/*"),
        ("c-ts", "test", "task_inputs", "ts/packages/proto/package.json"),
        ("app-ts", "test", "task_input_globs", "ts/packages/core/testing/**/*"),
        ("c-ts", "test", "task_inputs", "rs/crates/bindings/wb/wb_bg.wasm"),
        ("c-ts", "test", "task_inputs", "rs/crates/bindings/wb/wb.js"),
        ("c-ts", "test", "task_inputs", "ts/packages/kernel/tsconfig.json"),
        ("k-ts", "test", "task_inputs", "ts/packages/kernel/tsconfig.json"),
        ("k-ts", "test", "task_inputs", TS_BASE_TSCONFIG),
        ("c-ts", "test", "task_inputs", "ts/packages/kernel/src/wasm.ts"),
    ):
        broken = _a12_copy(a13)
        broken[pid][bucket][task].remove(entry)
        rows = _a13(broken)
        if f"{pid}:{task} inputs omit {entry}" not in rows:
            failures.append(f"A13 did not demand {entry} of {pid}:{task}")
        if pid == "c-ts" and any(r.startswith("c-ts:test-e2e ") for r in rows):
            failures.append(f"A13 blamed c-ts:test-e2e for {entry}, a shortfall that lives on c-ts:test")

    # T3. An untracked alias target: on a task that is not an FFI task, and on an FFI task that builds
    # somewhere else. The FFI task WITH the matching --out-dir is the clean row above.
    files = dict(a13_files)
    files["ts/packages/core/vitest.config.ts"] = "export default { resolve: { alias: { '@paigasus/kernel': '../kernel/.out/k.js' } } };\n"
    if (
        "c-ts:test aliases @paigasus/kernel to the untracked path ts/packages/kernel/.out/k.js, but the task does not "
        "build it there (no FFI build with --out-dir ts/packages/kernel/.out)"
    ) not in _a13(files=files):
        failures.append("A13 accepted an untracked alias target on a task that is not an FFI task")
    broken = _a12_copy(a13)
    broken["k-ts"]["invocations"]["test"] = a13_ffi.replace("--out-dir .out", "--out-dir other")
    if (
        "k-ts:test aliases @paigasus/wasm to the untracked path rs/crates/bindings/wb/.out/wb.js, but the task does "
        "not build it there (no FFI build with --out-dir rs/crates/bindings/wb/.out)"
    ) not in _a13(broken):
        failures.append("A13 accepted an untracked alias target that the FFI task builds somewhere else")

    # T4. An alias to a @paigasus/ package outside the closure. (An alias inside the own package is
    # the clean row's `server-only`, which demands nothing.)
    files = dict(a13_files)
    files["ts/packages/core/vitest.config.ts"] = "export default { resolve: { alias: { '@paigasus/ghost': '../kernel/src/wasm.ts' } } };\n"
    if (
        "c-ts:test aliases @paigasus/ghost in ts/packages/core/vitest.config.ts, but @paigasus/ghost is not in its "
        "package.json closure"
    ) not in _a13(files=files):
        failures.append("A13 accepted an alias to a @paigasus/ package outside the closure")

    # T4b. An alias target that leaves the repository: a relative path with too many `..`, and an
    # absolute string value.
    for a13_label, a13_value in (("a relative path that leaves the root", "../../../../../x.js"), ("an absolute path", "/etc/x.js")):
        files = dict(a13_files)
        files["ts/packages/core/vitest.config.ts"] = f"export default {{ resolve: {{ alias: {{ '@paigasus/kernel': '{a13_value}' }} }} }};\n"
        if not any(r.endswith("which is outside the repository") for r in _a13(files=files)):
            failures.append(f"A13 did not report {a13_label} as outside the repository")
    # T7. The proto dep, for a closure reader and for proto itself.
    broken = _a12_copy(a13)
    broken["app-ts"]["tasks"]["test"] = []
    broken["p-ts"]["tasks"]["test"] = []
    rows = _a13(broken)
    for target in ("app-ts:test", "p-ts:test"):
        if not any(r.startswith(f"{target} deps omit {PROTO_GENERATE_DEP}") for r in rows):
            failures.append(f"A13 did not demand {PROTO_GENERATE_DEP} of {target}")
    # T7. The task floor: no task matches vitest, so every per-task row goes quiet by itself.
    broken = _a12_copy(a13)
    for proj in broken.values():
        proj["invocations"] = dict.fromkeys(proj["invocations"], "pnpm exec jest")
    if not any(r.startswith("FLOOR: k-ts:test is not matched by the vitest token") for r in _a13(broken)):
        failures.append("A13's task floor did not fire when no task matches vitest")
    # T7. The alias floor: the kernel config loses one binding alias.
    files = dict(a13_files)
    files["ts/packages/kernel/vitest.config.ts"] = files["ts/packages/kernel/vitest.config.ts"].replace(
        "{ resolve: { alias: { '@paigasus/node-bindings': nb } } },", "{},"
    )
    if not any(r.startswith("FLOOR: k-ts:test no longer aliases @paigasus/node-bindings") for r in _a13(files=files)):
        failures.append("A13's alias floor did not fire when the kernel config lost an alias")
    # T7. A task with no input bucket is a violation, never a skip.
    broken = _a12_copy(a13)
    del broken["c-ts"]["task_input_globs"]["test"]
    if not any(r.startswith("c-ts:test reported no `inputFiles`/`inputGlobs`") for r in _a13(broken)):
        failures.append("A13 skipped a task that reported no input bucket")
    # T7. A None invocation is moon telling us nothing. That is infra, exactly as in A5 and A12.
    broken = _a12_copy(a13)
    broken["c-ts"]["invocations"]["test"] = None
    try:
        derive_vitest_tasks(broken)
    except MoonOutputError:
        pass
    else:
        failures.append("derive_vitest_tasks accepted a task with no command, script or args")

    # T8. Binding `files` shapes. Each is a row, never a skip.
    for wb_manifest, want_row in (
        ({"name": "@paigasus/wasm"},
         "rs/crates/bindings/wb/package.json has no `files` key, so A13 cannot tell which files vitest loads from the binding"),
        ({"name": "@paigasus/wasm", "files": "wb.js"}, "rs/crates/bindings/wb/package.json `files` is not a list"),
        ({"name": "@paigasus/wasm", "files": [1]},
         "rs/crates/bindings/wb/package.json `files` holds an entry of type int, not a string"),
        ({"name": "@paigasus/wasm", "files": ["*.js"]},
         "rs/crates/bindings/wb/package.json `files` entry '*.js' holds a glob character; A13 needs literal entries"),
    ):
        files = dict(a13_files)
        files["rs/crates/bindings/wb/package.json"] = json.dumps(wb_manifest)
        if want_row not in _a13(files=files):
            failures.append(f"A13 did not report {want_row!r}")
    files = dict(a13_files)
    files["rs/crates/bindings/wb/wb_bg.wasm"] = None
    files["rs/crates/bindings/wb/wb_bg.wasm/inner"] = ""
    if "rs/crates/bindings/wb/package.json `files` entry 'wb_bg.wasm' is a directory; A13 needs file entries" not in _a13(files=files):
        failures.append("A13 accepted a `files` entry that is a directory")

    # T9. tsconfig `extends` forms A13 does not follow are rows.
    for extends, shown in (
        ('"@paigasus/next-config/tsconfig-app"', "'@paigasus/next-config/tsconfig-app'"),
        ('["../../tsconfig.base.json"]', "['../../tsconfig.base.json']"),
    ):
        files = dict(a13_files)
        files["ts/packages/core/tsconfig.json"] = '{"extends": ' + extends + "}\n"
        want_row = (
            f"ts/packages/core/tsconfig.json: `extends` is {shown}; A13 follows only a relative path, so add a "
            f"rule for this form on purpose (SMA-736)"
        )
        if want_row not in _a13(files=files):
            failures.append(f"A13 did not report the `extends` form {extends}")
    # T9. `tsconfig: false` is what skips the app tsconfig: without it, A13 reads the file.
    files = dict(a13_files)
    files["ts/apps/app/vitest.config.ts"] = "export default {};\n"
    if not any(r.startswith("ts/apps/app/tsconfig.json: `extends` is '@paigasus/next-config/tsconfig-app'") for r in _a13(files=files)):
        failures.append("A13 did not read the app tsconfig once its config stopped setting `tsconfig: false`")
    # T9. EVERY config must set it: one of two is not enough.
    files = dict(a13_files)
    files["ts/apps/app/vitest.other.config.ts"] = "export default {};\n"
    broken = _a12_copy(a13)
    broken["app-ts"]["invocations"]["test"] = "pnpm exec vitest run && pnpm exec vitest run --config vitest.other.config.ts"
    broken["app-ts"]["task_inputs"]["test"].append("ts/apps/app/vitest.other.config.ts")
    if not any(r.startswith("ts/apps/app/tsconfig.json: `extends`") for r in _a13(broken, files=files)):
        failures.append("A13 skipped the tsconfig of a task where only one of two configs sets `tsconfig: false`")
    # T9. A task with NO config file still reads its tsconfig ("every config" is not vacuously true).
    files = dict(a13_files)
    files["ts/packages/proto/tsconfig.json"] = '{"extends": "../../tsconfig.base.json"}\n'
    if "p-ts:test inputs omit ts/packages/proto/tsconfig.json" not in _a13(files=files):
        failures.append("A13 skipped the tsconfig of a task that has no vitest config file")

    # T10. The gate reachability check.
    broken = _a12_copy(a13)
    broken["repo"]["task_input_globs"]["affected-smoke"] = ["ts/packages/*/vitest.config.ts"]
    rows = _a13(broken)
    for cfg in ("ts/packages/core/vitest.e2e.config.ts", "ts/apps/app/vitest.config.ts"):
        if f"{cfg} matches no input of repo:affected-smoke, so an edit to it alone does not schedule the gate that reads it" not in rows:
            failures.append(f"A13 did not report that {cfg} cannot schedule repo:affected-smoke")
    # A negated glob excludes a config that a positive glob matches (coordinator ruling, SMA-736).
    a13_neg_row = "ts/apps/app/vitest.config.ts matches no input of repo:affected-smoke, so an edit to it alone does not schedule the gate that reads it"
    broken = _a12_copy(a13)
    broken["repo"]["task_input_globs"]["affected-smoke"] = [*broken["repo"]["task_input_globs"]["affected-smoke"], "!ts/apps/app/vitest.config.ts"]
    if a13_neg_row not in _a13(broken):
        failures.append("A13 counted a config as reachable although a negated glob of repo:affected-smoke excludes it")
    broken = _a12_copy(a13)
    del broken["repo"]["task_input_globs"]["affected-smoke"]
    if not any(r.startswith("repo:affected-smoke reported no `inputGlobs`") for r in _a13(broken)):
        failures.append("A13 skipped the reachability check when repo:affected-smoke reported no globs")

    # T12. A broken manifest prints once. With no `tsc` task, A13 reports a broken binding manifest.
    # With a `tsc` task whose closure reaches it, A12a reports it and A13 drops it.
    files = dict(a13_files)
    files["rs/crates/bindings/wb/package.json"] = "{"
    if not any(r.startswith("rs/crates/bindings/wb/package.json is not valid JSON") for r in _a13(files=files)):
        failures.append("A13 did not report a broken manifest that no `tsc` task reads")
    broken = _a12_copy(a13)
    broken["c-ts"]["invocations"]["typecheck"] = "pnpm exec tsc -p tsconfig.json --noEmit"
    broken["c-ts"]["tasks"]["typecheck"] = [PROTO_GENERATE_DEP]
    broken["c-ts"]["task_inputs"]["typecheck"] = []
    broken["c-ts"]["task_input_globs"]["typecheck"] = []
    if any(r.startswith("rs/crates/bindings/wb/package.json") for r in _a13(broken, files=files)):
        failures.append("A13 printed a broken manifest again that A12a already reports")

    # A13 (SMA-736 §4.7, T11) — the real-corpus pin. Every tracked vitest config is parsed, and the
    # result must equal this literal table of (config, sorted aliases, tsconfig_off). A parser change
    # that alters what it finds in a real config reds here, and so does a new config file: add it to
    # the table on purpose. Spec R1.
    a13_corpus = (
        ("ts/apps/gateway-console/vitest.config.ts", (
            ("next/cache", "ts/apps/gateway-console/tests/support/next-cache.ts"),
            ("next/headers", "ts/apps/gateway-console/tests/support/next-headers.ts"),
            ("server-only", "ts/apps/gateway-console/tests/support/server-only-stub.ts"),
        ), True),
        ("ts/apps/iam-console/vitest.config.ts", (
            ("next/cache", "ts/apps/iam-console/tests/support/next-cache.ts"),
            ("next/headers", "ts/apps/iam-console/tests/support/next-headers.ts"),
            ("server-only", "ts/apps/iam-console/tests/support/server-only-stub.ts"),
        ), True),
        ("ts/packages/paigasus-app-shell/vitest.config.ts", (), False),
        ("ts/packages/paigasus-auth/vitest.config.ts", (
            ("server-only", "ts/packages/paigasus-auth/tests/support/server-only-stub.ts"),
        ), False),
        ("ts/packages/paigasus-auth/vitest.containers.config.ts", (
            ("server-only", "ts/packages/paigasus-auth/tests/support/server-only-stub.ts"),
        ), False),
        ("ts/packages/paigasus-console-core/vitest.config.ts", (
            ("next/cache", "ts/packages/paigasus-console-core/tests/support/next-cache.ts"),
            ("next/headers", "ts/packages/paigasus-console-core/tests/support/next-headers.ts"),
            ("server-only", "ts/packages/paigasus-console-core/tests/support/server-only-stub.ts"),
        ), False),
        ("ts/packages/paigasus-console-core/vitest.containers.config.ts", (
            ("server-only", "ts/packages/paigasus-console-core/tests/support/server-only-stub.ts"),
        ), False),
        ("ts/packages/paigasus-discovery/vitest.config.ts", (
            ("server-only", "ts/packages/paigasus-discovery/tests/support/server-only-stub.ts"),
        ), False),
        ("ts/packages/paigasus-discovery/vitest.containers.config.ts", (
            ("server-only", "ts/packages/paigasus-discovery/tests/support/server-only-stub.ts"),
        ), False),
        ("ts/packages/paigasus-kernel/vitest.config.ts", (
            ("@paigasus/node-bindings", "rs/crates/bindings/paigasus-node-bindings/index.js"),
            ("@paigasus/wasm", "rs/crates/bindings/paigasus-wasm/.wasmpack-test-out/paigasus_wasm.js"),
        ), False),
        ("ts/packages/paigasus-next-config/vitest.config.ts", (), False),
        ("ts/packages/paigasus-sdk/vitest.config.ts", (), False),
        ("ts/packages/paigasus-ui/vitest.config.ts", (), False),
    )
    a13_corpus_root = Path(__file__).resolve().parents[2]
    try:
        a13_corpus_tracked = task_inputs.tracked_files(a13_corpus_root)
    except INFRA_ERRORS as exc:
        # Mirror main(): a broken `git` is an infrastructure error (rc 2), not a graph regression.
        print(f"FATAL [parity] could not list the tracked files: {exc}", file=sys.stderr)
        return 2
    a13_corpus_paths = sorted(p for p in a13_corpus_tracked if re.search(r"(^|/)vitest[^/]*\.config\.[^/]+$", p))
    if a13_corpus_paths != [cfg for cfg, _aliases, _off in a13_corpus]:
        failures.append(
            f"the tracked vitest configs are {a13_corpus_paths}, but the A13 corpus pin lists "
            f"{[cfg for cfg, _aliases, _off in a13_corpus]} — add a new config to the pin on purpose"
        )
    for cfg, want_aliases, want_off in a13_corpus:
        rows = []
        path = a13_corpus_root / cfg
        if not path.is_file():
            failures.append(f"the A13 corpus pin lists {cfg}, but it is not a file in the tree, so the parser cannot read it")
            continue
        aliases, off = vitest_config_facts(_read_utf8(path), cfg, rows)
        if rows or tuple(aliases) != want_aliases or off is not want_off:
            failures.append(
                f"the A13 parser reads {cfg} as aliases {aliases}, tsconfig_off {off}, rows {rows}; the "
                f"corpus pin says aliases {list(want_aliases)}, tsconfig_off {want_off}, no rows"
            )
    # A14 (SMA-765) - the uv parser, called directly. Spacing varies on purpose (tab, two spaces, a
    # `\` continuation, global options), so the rows do not inherit the implementer's assumption.
    for a14_label, a14_text, a14_want_red in (
        ("uv sync", "uv sync", True),
        ("two spaces", "uv  sync", True),
        ("a tab", "uv\tsync", True),
        ("a global flag", "uv -q sync", True),
        ("a valued global option", "uv --project py sync", True),
        ("an unlisted valued option", "uv --allow-insecure-host h sync", True),
        ("--python-preference", "uv --python-preference only-managed sync", True),
        ("--python", "uv --python 3.12 sync", True),
        ("a line continuation", "uv \\\n  sync", True),
        ("sync --inexact", "uv sync --inexact", True),
        ("pip sync", "uv pip sync r.txt", True),
        ("pip install", "uv pip install x", True),
        ("pip uninstall", "uv pip uninstall x", True),
        ("venv", "uv venv", True),
        ("add", "uv add x", True),
        ("remove", "uv remove x", True),
        ("run --reinstall-package", "uv run --reinstall-package p pytest", True),
        ("run --reinstall", "uv run --reinstall pytest", True),
        ("run --exact", "uv run --exact pytest", True),
        ("run --force-reinstall", "uv run --force-reinstall pytest tests", True),
        ("not an approved form: isolated run --force-reinstall", "uv run --isolated --force-reinstall pytest", True),
        ("version --bump", "uv version --bump patch", True),
        ("version positional", "uv version 1.2.3", True),
        ("not an approved form: bare version", "uv version", True),
        ("not an approved form: version --short", "uv version --short", True),
        ("not an approved form: version --output-format", "uv version --output-format json", True),
        ("not an approved form: version --bump --no-sync", "uv version --bump patch --no-sync", True),
        ("not an approved form: version --bump --dry-run", "uv version --bump patch --dry-run", True),
        ("not an approved form: version --bump --frozen", "uv version --bump patch --frozen", True),
        ("--project value is a verb name", "uv --project run sync", True),
        ("--directory value is a verb name", "uv --directory add sync", True),
        ("not an approved form: --directory value hides run", "uv --directory sync run pytest", True),
        ("--isolated is the executed command's", "uv run --reinstall-package p echo --isolated", True),
        ("--isolated in a comment", "uv run --reinstall-package p pytest # --isolated", True),
        ("mutating flag after a valued option", "uv run --with x --reinstall pytest", True),
        ("not an approved form: isolated with a valued option", "uv run --isolated --with x --reinstall-package p pytest", True),
        ("not an approved form: isolated, reinstall-package with another package", "uv run --isolated --reinstall-package p pytest tests", True),
        ("a comment is not executed", "uv run --locked pytest # --reinstall", False),
        ("not an approved form: --no-install-project", "uv run --no-install-project --reinstall-package p --isolated pytest", True),
        ("not an approved form: --no-build", "uv run --no-build --reinstall-package p --isolated pytest", True),
        ("an escaped space keeps # in the word", "uv run --env-file a\\ #settings.env --reinstall pytest", True),
        ("a quoted space keeps # in the word", 'uv run --env-file "a #settings.env" --reinstall pytest', True),
        ("an unbalanced quote fails closed", 'uv run --env-file "a --reinstall pytest', True),
        ("an unbalanced quote gives no --isolated credit", 'uv run --isolated --reinstall-package p "pytest', True),
        ("not an approved form: a prose apostrophe", "echo uv run's own status", True),
        ("# inside a word is not a comment", "uv run --env-file a#b --reinstall pytest", True),
        ("a single-quoted # is a value", "uv run --env-file '#settings.env' --reinstall pytest", True),
        ("a double-quoted # is a value", 'uv run --env-file "#settings.env" --reinstall pytest', True),
        ("run --refresh-package", "uv run --refresh-package p pytest", True),
        ("UV_PROJECT_ENVIRONMENT assignment", "UV_PROJECT_ENVIRONMENT=.venv uv run pytest", True),
        ("export VIRTUAL_ENV", "export VIRTUAL_ENV=x", True),
        ("unset UV_PROJECT_ENVIRONMENT", "unset UV_PROJECT_ENVIRONMENT", True),
        ("--active", "uv run --active x", True),
        ("a string is not stripped", 'echo "run uv sync"', True),
        ("--isolated in a different invocation",
         "uv run --isolated true && uv run --reinstall-package p pytest", True),
        ("the target form", "uv run --isolated --reinstall-package paigasus-py-bindings pytest tests", False),
        ("plain run needs a lock flag (R-relock)", "uv run pytest", True),
        ("plain run with a valued option (R-relock)", "uv run --project py python3 x", True),
        ("run --isolated alone", "uv run --isolated pytest", False),
        ("run --locked", "uv run --locked --project py ruff", False),
        ("run --frozen", "uv run --frozen x", False),
        ("not an approved form: a prefix and another package",
         "touch a b && uv run --isolated --reinstall-package p pytest tests", True),
        ("uv.lock is not uv", "cat py/uv.lock", False),
        ("a continuation splits uv", "u\\\nv sync", True),
        ("a continuation splits VIRTUAL_ENV", "VIRTUAL\\\n_ENV=x", True),
        ("a continuation splits --active", "--act\\\nive", True),
        ("an unknown verb", "uv frobnicate --sync", True),
        ("a := default of VIRTUAL_ENV", "echo ${VIRTUAL_ENV:=x}", True),
        ("a := default of UV_PROJECT_ENVIRONMENT", ': "${UV_PROJECT_ENVIRONMENT:=.venv-x}"', True),
        ("an assignment with no command", "UV_PROJECT_ENVIRONMENT=.venv-x", True),
        ("an assignment, then uv run", "VIRTUAL_ENV=/x\nuv run pytest", True),
        ("uv lock", "uv lock", True),
        ("run --upgrade", "uv run --upgrade pytest", True),
        ("tool install", "uv tool install ruff", True),
        ("uv in a variable", 'UVB=uv; "$UVB" sync', True),
        ("a comment line does not join the next line", "# uv run --isolated \\\nuv sync", True),
        ("a later # is not a comment", "x # uv sync", True),
        ("a quoted command word", 'uv run --locked "--reinstall" pytest', True),
        ("a redirect as command word", "uv run --locked >/dev/null --reinstall pytest", True),
        ("a stderr redirect as command word", "uv run --locked 2>/dev/null --upgrade pytest", True),
        ("a $ command word", "uv run --locked $F pytest", True),
        ("--python 3.13", "uv run --python 3.13 pytest", True),
        ("--project with an option as value", "uv run --project --reinstall pytest", True),
        ("--project with a quoted pair", 'uv run --project "a b" --reinstall pytest', True),
        ("--project with a substitution", "uv run --project $(x) --reinstall pytest", True),
        ("--project=py", "uv run --project=py pytest", True),
        ("run with no command", "uv run --locked", True),
        ("build with a flag", "uv build --wheel", True),
        ("a second uv in bash -c", 'bash -c "uv run --locked pytest; uv sync"', True),
        ("reinstall without --isolated", "uv run --reinstall-package paigasus-py-bindings pytest", True),
        ("command uv without -v", "command uv sync", True),
        ("command uv as a redirect word", "command uv >/dev/null", True),
        ("X=--active", "X=--active", True),
        ("UV_PYTHON assignment", "UV_PYTHON=3.13 uv run pytest", True),
        ("python heredoc data", "python3 - <<'EOF'\nimport subprocess\nsubprocess.run([\"uv\", \"sync\"])\nEOF", True),
        ("a CR at line end", "uv sync\r", True),
        ("echo uv", "echo uv", True),
        ("uv build", "uv build", False),
        ("uv build and a tail", "uv build && echo ok", False),
        ("run, project py", "uv run --locked --project py python3 -c x", False),
        ("run, project ci/release-plan", "uv run --locked --project ci/release-plan --python '>=3.12' python3 x", False),
        ("run, project HERE", "uv run --locked --project \"$HERE\" --python '>=3.12' python3 x", False),
        ("run --frozen python", "uv run --frozen python -c x", False),
        ("command -v uv", "command -v uv >/dev/null 2>&1", False),
        ("command -v uv and a tail", "command -v uv >/dev/null 2>&1 || exit 2", False),
        ("the floor form after a prefix",
         "touch a b && uv run --isolated --reinstall-package paigasus-py-bindings pytest tests", False),
        ("reinstall before --isolated",
         "uv run --reinstall-package paigasus-py-bindings --isolated pytest tests", False),
        ("a continuation", "uv run \\\n  --locked pytest", False),
        ("a comment line", "# uv sync", False),
        ("a CR before # is not a comment", "\r# x; uv sync", True),
        ("a form feed before # is not a comment", "\x0c# x; uv sync", True),
        ("a NBSP before # is not a comment", "\xa0# x; uv sync", True),
        ("a tab before # is a comment", "\t# uv sync", False),
        # Follow-up sweep 2. R3: a command word that installs into the environment.
        ("R3 uv run pip", "uv run --locked pip install x", True),
        ("R3 uv run pip3 by path", "uv run --locked bin/pip3 list", True),
        ("R3 uv run virtualenv", "uv run --locked virtualenv x", True),
        ("R3 uv run python -m pip", "uv run --locked python -m pip install x", True),
        ("R3 uv run python3 -mpip", "uv run --locked python3 -mpip install x", True),
        ("R3 uv run python -m venv", "uv run --frozen python3.12 -m venv x", True),
        ("R3 uv run python -m pytest", "uv run --locked python -m pytest", False),
        ("R3 an interpreter option before -m", "uv run --locked python -I -m ensurepip", True),
        ("R3 two interpreter options before -mpip", "uv run --locked python3 -I -B -mpip list", True),
        ("R4 -m venv", "python3 -I -m venv --clear py/.venv-x", True),
        # R2: `eval`, a captured probe, and spellings that a shell turns into `uv`.
        ("R2 eval", 'eval "$CMD"', True),
        ("R2 evaluate is a word", "evaluate x", False),
        ("R2 captured probe", "UVB=$(command -v uv)", True),
        ("R2 captured probe, backtick", "UVB=`command -v uv`", True),
        ("R2 ${x}v", "${x}v sync", True),
        ("R2 $(echo u)v", "$(echo u)v sync", True),
        ("R2 u${x}", "u${x} sync", True),
        ("R2 u$x", "u$x sync", True),
        ("R2 a backslash", "u\\v sync", True),
        ("R2 two single quotes", "u''v sync", True),
        ("R2 a quoted u", '"u"v sync', True),
        ("R2 no uv in ${x}y", "echo ${x}y", False),
        ("R2 menu$x is not u$x", "menu$x", False),
        # R4: a write to an environment that does not go through `uv`.
        ("R4 pip install", "pip install x", True),
        ("R4 pip3 uninstall", "pip3 uninstall -y x", True),
        ("R4 $PY -m pip", '"$PY" -m pip install x', True),
        ("R4 -m ensurepip", "python3 -m ensurepip", True),
        ("R4 .venv/bin/pip", "py/.venv/bin/pip list", True),
        ("R4 .venv/bin/python -m pip", "py/.venv/bin/python3 -m pip freeze", True),
        ("R4 rm .venv", "rm -rf py/.venv", True),
        ("R4 mv .venv", "mv py/.venv /tmp/x", True),
        ("R4 $PY -m pytest", '"$PY" -m pytest', False),
        ("R4 cat a .venv file", "cat py/.venv/pyvenv.cfg", False),
        ("R4 pip in a word", "shellcheck-pip install", False),
        # R1: a `#` line is a comment only when no quote is open from an earlier line.
        ("R1 a # line inside an open quote", "bash -c '\n#'; uv sync", True),
        ("R1 an escaped space keeps # mid-word", "echo a\\ # '\n#'; uv sync", True),
        ("R1 a continuation keeps # mid-word", "echo a\\\n# '\n#'; uv sync", True),
        ("R1 ; ends a word before #", "true;# '\necho '\n#'; uv sync", True),
        ("R2 a joined literal uv does not cancel u''v", "echo prefix\"uv build &&\"; u''v sync", True),
        ("R2 quotes elsewhere do not make a plain uv a spelling", 'X="a" uv build', False),
        ("R4 a pip option before install", "pip -q install x", True),
        ("R4 a valued pip option before install", "pip3 --index-url https://x install y", True),
        ("R4 pip list", "pip list", False),
        ("R3 a version-qualified pip", "uv run --locked pip3.12 list", True),
        ("R4 a version-qualified pip install", "pip3.12 install x", True),
        ("R4 pip.conf is not pip", "cat pip.conf install", False),
        ("R1 a # line inside open double quotes", 'bash -c "\n#"; uv sync', True),
        ("R1 a # line after a closed quote", "echo 'x'\n# uv sync", False),
        ("R1 quotes inside $( ) in double quotes", "X=\"$(grep 'a=\"$(b)\"' f)\"\n# uv sync", False),
        ("R1 a # line inside an open $( )", "X=$(\n# uv sync\ntrue)", False),
        ("R1 a single-quoted \" inside $( ) in double quotes", "X=\"$(echo 'a\"b')\"\n# uv sync", False),
        ("uv in a command substitution",
         "PY=\"$(uv run --locked --project \"$HERE\" python3 -c 'import sys; print(sys.executable)')\"", False),
    ):
        if bool(uv_mutations(a14_text)) != a14_want_red:
            failures.append(f"A14 parser, {a14_label}: {a14_text!r} should be {'red' if a14_want_red else 'green'}")

    # A14 - the check. Small project dicts in the shape of the A5 fixture.
    a14_smoke_globs = ["ci/**/*", "py/scripts/**/*"]

    def a14_proj(blob, env=None, source_dir="py", smoke_globs=a14_smoke_globs, name="p"):
        projects = {name: {"source_dir": source_dir, "deps": {}, "tasks": {"test": []},
                           "task_inputs": {"test": []}, "invocations": {"test": blob},
                           "task_env": {"test": env or {}}}}
        if smoke_globs is not None:
            projects[AFFECTED_SMOKE_PROJECT] = {
                "source_dir": ".", "deps": {}, "tasks": {AFFECTED_SMOKE_TASK: []},
                "task_inputs": {AFFECTED_SMOKE_TASK: []},
                "task_input_globs": {AFFECTED_SMOKE_TASK: smoke_globs}}
        return projects

    a14_floor = ("p:test",)
    a14_good = "uv run --isolated --reinstall-package paigasus-py-bindings pytest tests"
    with tempfile.TemporaryDirectory() as a14_tmp:
        a14_root = Path(a14_tmp)
        (a14_root / "ci" / "x").mkdir(parents=True)
        (a14_root / "py" / "scripts").mkdir(parents=True)
        (a14_root / "ci" / "x" / "run.sh").write_text("#!/bin/bash\nuv sync\n")
        (a14_root / "ci" / "x" / "msg.sh").write_text("#!/bin/bash\necho 'run uv sync'\n")
        (a14_root / "py" / "scripts" / "run_tests.sh").write_text("#!/bin/bash\nuv pip install x\n")

        def a14_run(projects, **kw):
            kw.setdefault("allow", {})
            kw.setdefault("floor", a14_floor)
            return check_uv_env_isolation(projects, a14_root, **kw)

        (a14_root / "py" / "scripts" / "latin.sh").write_bytes(bytes([0x75, 0x76, 0x20, 0xff, 0x0a]))
        try:
            a14_run(a14_proj(a14_good + " && bash scripts/latin.sh"))
            a14_latin = None
        except MoonOutputError as a14_exc:
            a14_latin = str(a14_exc)
        if a14_latin is None or "latin.sh" not in a14_latin or "not valid UTF-8" not in a14_latin:
            failures.append("A14 did not raise MoonOutputError on a script that is not UTF-8")
        if a14_run(a14_proj(a14_good)):
            failures.append(f"A14 reported rows on the clean floor task: {a14_run(a14_proj(a14_good))}")
        if not any(r.startswith("FLOOR:") and "p:test" in r for r in a14_run({}, floor=a14_floor)):
            failures.append("A14 did not report a FLOOR row for a missing floor task")
        a14_rows = a14_run(a14_proj("uv run --reinstall-package paigasus-py-bindings pytest tests"))
        if not any("p:test" in r and not r.startswith("FLOOR:") for r in a14_rows):
            failures.append(f"A14 did not red a floor task without --isolated: {a14_rows}")
        if not any(r.startswith("FLOOR:") for r in a14_rows):
            failures.append(f"A14 gave no FLOOR row for a floor task without --isolated: {a14_rows}")
        a14_rows = a14_run(a14_proj("uv run --reinstall-package paigasus-py-bindings pytest --isolated"))
        if not any(r.startswith("FLOOR:") for r in a14_rows):
            failures.append(f"A14 gave no FLOOR row when --isolated is outside the option region: {a14_rows}")
        a14_rows = a14_run(a14_proj("uv run --isolated pytest tests"))
        if not any(r.startswith("FLOOR:") for r in a14_rows):
            failures.append(f"A14 gave no FLOOR row without --reinstall-package paigasus-py-bindings: {a14_rows}")
        a14_rows = a14_run(a14_proj("uv run --reinstall-package paigasus-py-bindings --isolated pytest tests"))
        if any(r.startswith("FLOOR:") for r in a14_rows):
            failures.append(f"A14 gave a FLOOR row when --isolated follows --reinstall-package: {a14_rows}")
        for a14_var in ("UV_PROJECT_ENVIRONMENT", "VIRTUAL_ENV"):
            if not a14_run(a14_proj(a14_good, env={a14_var: ".venv-x"})):
                failures.append(f"A14 did not red a task env that sets {a14_var}")
        if a14_run(a14_proj(a14_good, env={"OTHER": "1"})):
            failures.append("A14 reddened an unrelated task env key")
        a14_rows = a14_run(a14_proj(a14_good, env={"UV_PYTHON": "3.13"}))
        if not any("p:test" in r and "UV_PYTHON" in r for r in a14_rows):
            failures.append(f"A14 did not red a task env that sets UV_PYTHON: {a14_rows}")
        (a14_root / "ci" / "version-lockstep").mkdir()
        (a14_root / "ci" / "version-lockstep" / "run.sh").write_text("#!/bin/bash\ntrue\n")
        a14_rows = a14_run(a14_proj(a14_good + " && bash ci/version-lockstep/run.sh --write"))
        if not any("runs ci/version-lockstep/run.sh with --write" in r for r in a14_rows):
            failures.append(f"A14 did not red a version-lockstep task with --write: {a14_rows}")
        for a14_ok in (" && bash ci/version-lockstep/run.sh --self-test",
                       " && bash ci/version-lockstep/run.sh && echo --write"):
            if any("runs ci/version-lockstep/run.sh with --write" in r for r in a14_run(a14_proj(a14_good + a14_ok))):
                failures.append(f"A14 reddened a version-lockstep task without --write: {a14_ok!r}")
        a14_rows = a14_run(a14_proj(a14_good + " && bash ci/x/run.sh"))
        if not any("ci/x/run.sh" in r for r in a14_rows):
            failures.append(f"A14 did not red a `uv sync` in a followed ci script: {a14_rows}")
        a14_rows = a14_run(a14_proj(a14_good + " && bash scripts/run_tests.sh"))
        if not any("py/scripts/run_tests.sh" in r for r in a14_rows):
            failures.append(f"A14 did not red a `uv pip install` in a project scripts/*.sh: {a14_rows}")
        a14_msg = ("ci/x/msg.sh", "echo 'run uv sync'")
        a14_msg_proj = a14_proj(a14_good + " && bash ci/x/msg.sh")
        if not a14_run(a14_msg_proj):
            failures.append("A14 did not red a message line that is not allowlisted")
        if a14_run(a14_msg_proj, allow={a14_msg: "prints a remedy only"}):
            failures.append(f"A14 reddened an allowlisted message line: {a14_run(a14_msg_proj, allow={a14_msg: 'r'})}")
        if not a14_run(a14_msg_proj, allow={a14_msg: "  "}):
            failures.append("A14 accepted an allowlist entry with an empty reason")
        (a14_root / "ci" / "x" / "cont.sh").write_text("echo \"run 'uv sync' now\" \\\n  >&2\n")
        a14_cont = ("ci/x/cont.sh", "echo \"run 'uv sync' now\" \\")
        a14_cont_proj = a14_proj(a14_good + " && bash ci/x/cont.sh")
        if a14_run(a14_cont_proj, allow={a14_cont: "PROSE: test"}):
            failures.append(f"A14 did not match a waiver for a line that ends in a backslash: {a14_run(a14_cont_proj, allow={a14_cont: 'PROSE: test'})}")
        if not any("ci/x/cont.sh:1" in r for r in a14_run(a14_cont_proj)):
            failures.append(f"A14 row does not name the physical line of a continued match: {a14_run(a14_cont_proj)}")
        # G5: every file A14 reads must match an input of repo:affected-smoke (SMA-768).
        (a14_root / "other" / "scripts").mkdir(parents=True)
        (a14_root / "other" / "scripts" / "x.sh").write_text("#!/bin/bash\ntrue\n")
        a14_g5 = a14_run(a14_proj("bash scripts/x.sh", source_dir="other", name="q"), floor=())
        if not any("other/scripts/x.sh" in r and "matches no input" in r for r in a14_g5):
            failures.append(f"A14 did not red a read script that no affected-smoke input matches: {a14_g5}")
        a14_g5 = a14_run(a14_proj("bash scripts/x.sh", source_dir="other", name="q",
                                  smoke_globs=[*a14_smoke_globs, "other/**/*", "!other/scripts/x.sh"]), floor=())
        if not any("other/scripts/x.sh" in r and "matches no input" in r for r in a14_g5):
            failures.append(f"A14 did not red a read script that a negated smoke glob removes: {a14_g5}")
        a14_g5 = a14_run(a14_proj("bash scripts/x.sh", source_dir="other", name="q",
                                  smoke_globs=[*a14_smoke_globs, "other/**/*"]), floor=())
        if a14_g5:
            failures.append(f"A14 reddened a read script that an affected-smoke glob matches: {a14_g5}")
        a14_g5 = a14_run(a14_proj("bash scripts/x.sh", source_dir="other", name="q", smoke_globs=None), floor=())
        if not any("inputGlobs" in r for r in a14_g5):
            failures.append(f"A14 gave no row when repo:affected-smoke reports no inputGlobs: {a14_g5}")
        # Rule 8: a `\`+newline continuation, `--write=x` and `--check --write` all red.
        for a14_w in (" && bash ci/version-lockstep/run.sh \\\n  --write",
                      " && bash ci/version-lockstep/run.sh --write=x",
                      " && bash ci/version-lockstep/run.sh --check --write"):
            if not any("runs ci/version-lockstep/run.sh with --write" in r
                       for r in a14_run(a14_proj(a14_good + a14_w))):
                failures.append(f"A14 did not red a version-lockstep --write form: {a14_w!r}")
        # A split flag: bash removes `\`+newline with no space, so `--wr\`+newline+`ite` is `--write`.
        if not any("runs ci/version-lockstep/run.sh with --write" in r
                   for r in a14_run(a14_proj(a14_good + " && bash ci/version-lockstep/run.sh --wr\\\nite"))):
            failures.append("A14 did not red a version-lockstep --write that a continuation splits")
        # R10 follow-up: the cheap indirect forms red; a different script or no --write stays green.
        for a14_w, a14_want in (
            ("cd ci/version-lockstep && ./run.sh --write", True),
            ("cd ci/version-lockstep && bash run.sh --write", True),
            ("(cd ci/version-lockstep; ./run.sh --write)", True),
            ("{ cd ci/version-lockstep; ./run.sh --write; }", True),
            ("cd ci && bash version-lockstep/run.sh --write", True),
            ("pushd ci/version-lockstep && ./run.sh --write", True),
            ("cd ci/version-lockstep\n./run.sh --write", True),
            ("bash ci/version-lockstep/run.sh 'a;b' --write", True),
            ('bash ci/version-lockstep/run.sh "a && b" --write', True),
            ("D=ci/version-lockstep\nbash $D/run.sh --write", True),
            ('D=ci/version-lockstep\nbash "${D}/run.sh" --write', True),
            ('bash "$ROOT"/ci/version-lockstep/run.sh --write', True),
            ("cd ci/x && ./run.sh --write", False),
            ("cd ci/version-lockstep && ./run.sh --self-test", False),
            ('cd ci/version-lockstep && cd "$ROOT" && ./run.sh --write', True),
            ("cd ci/version-lockstep && (cd ../actionlint && true) && ./run.sh --write", True),
            ("D=ci/version-lockstep\nbash \"$D/run.sh\" --write\nD=ci/actionlint", True),
            ("cd cd ci/version-lockstep && ./run.sh --write", True),
            ('D=ci/version-lockstep; bash "$D/run.sh" --write', True),
            ('export D=ci/version-lockstep && bash "$D/run.sh" --write', True),
            ('export export D=ci/version-lockstep && bash "$D/run.sh" --write', True),
            ("pushd pushd ci/version-lockstep && ./run.sh --write", True),
            ("bash ci/version-lockstep/run.sh 'a' && echo --write", False),
        ):
            if runs_lockstep_write(a14_w) is not a14_want:
                failures.append(f"A14 rule 8 reads {a14_w!r} as {'green' if a14_want else 'red'}")
        if not any("runs ci/version-lockstep/run.sh with --write" in r
                   for r in a14_run(a14_proj(a14_good + " && cd ci/version-lockstep && ./run.sh --write"))):
            failures.append("A14 did not red `cd ci/version-lockstep && ./run.sh --write` through the check")
        # Follow-up sweep 2, R10: --write from a variable, from "$@", through moon, or after a computed path.
        for a14_w, a14_want in (
            ("W=--write\nbash ci/version-lockstep/run.sh $W", True),
            ('bash ci/version-lockstep/run.sh "$@"', True),
            ("bash ci/version-lockstep/run.sh $UNSET", True),
            ("bash ci/version-lockstep/run.sh `printf -- --write`", True),
            ("moon run repo:version-lockstep -- --write", True),
            ("moon run :version-lockstep -- --write", True),
            ("bash $(dirname x)/run.sh --write", True),
            ("bash `pwd`/run.sh --write", True),
            ("moon run repo:version-lockstep", False),
            ("moon run repo:version-lockstep-x -- --write", False),
            ("bash $(dirname x)/run.sh --check", False),
            ("W=--check\nbash ci/version-lockstep/run.sh $W", False),
        ):
            if runs_lockstep_write(a14_w) is not a14_want:
                failures.append(f"A14 rule 8 (R10) reads {a14_w!r} as {'green' if a14_want else 'red'}")
        (a14_root / "ci" / "x" / "lock.sh").write_text("#!/bin/bash\nbash ci/version-lockstep/run.sh --write\n")
        a14_rows = a14_run(a14_proj(a14_good + " && bash ci/x/lock.sh"))
        if not any("ci/x/lock.sh can run ci/version-lockstep/run.sh with --write" in r for r in a14_rows):
            failures.append(f"A14 did not red a scanned script that runs version-lockstep --write (R10): {a14_rows}")
        # R8: a script that a scanned script RUNS is scanned, and so is a task's own `*.sh` outside ci/.
        (a14_root / "ci" / "x" / "mut8.sh").write_text("#!/bin/bash\nuv sync\n")
        (a14_root / "ci" / "x" / "runner.sh").write_text("#!/bin/bash\nbash ci/x/mut8.sh\n")
        a14_rows = a14_run(a14_proj(a14_good + " && bash ci/x/runner.sh"))
        if not any("ci/x/mut8.sh" in r for r in a14_rows):
            failures.append(f"A14 did not scan a script that a scanned script runs (R8): {a14_rows}")
        (a14_root / "ci" / "x" / "here.sh").write_text('#!/bin/bash\nHERE="$(cd "$(dirname "$0")" && pwd)"\n"$HERE/mut8.sh"\n')
        a14_rows = a14_run(a14_proj(a14_good + " && bash ci/x/here.sh"))
        if not any("ci/x/mut8.sh" in r for r in a14_rows):
            failures.append(f"A14 did not scan a script that a scanned script runs by its own directory (R8): {a14_rows}")
        # R8: a directory variable that the script assigns is expanded (`D=varx; bash "$D/y.sh"`).
        (a14_root / "varx").mkdir()
        (a14_root / "varx" / "v8.sh").write_text("#!/bin/bash\nuv sync\n")
        (a14_root / "ci" / "x" / "var8.sh").write_text('#!/bin/bash\nD=varx\nbash "$D/v8.sh"\n')
        a14_rows = a14_run(a14_proj(a14_good + " && bash ci/x/var8.sh", smoke_globs=[*a14_smoke_globs, "varx/**/*"]))
        if not any("varx/v8.sh" in r for r in a14_rows):
            failures.append(f"A14 did not expand an assigned directory variable in a run reference (R8): {a14_rows}")
        # R8: a child script inherits the task's cwd, so `bash scripts/m.sh` resolves in the project.
        (a14_root / "py" / "scripts" / "child8.sh").write_text("#!/bin/bash\nuv sync\n")
        (a14_root / "py" / "scripts" / "parent8.sh").write_text("#!/bin/bash\nbash scripts/child8.sh\n")
        a14_rows = a14_run(a14_proj(a14_good + " && bash scripts/parent8.sh"))
        if not any("py/scripts/child8.sh" in r for r in a14_rows):
            failures.append(f"A14 did not resolve a child script against the task's cwd (R8): {a14_rows}")
        (a14_root / "ops").mkdir()
        (a14_root / "ops" / "y.sh").write_text("#!/bin/bash\nuv sync\n")
        a14_rows = a14_run(a14_proj(a14_good + " && bash ops/y.sh", smoke_globs=[*a14_smoke_globs, "ops/**/*"]))
        if not any("ops/y.sh" in r for r in a14_rows):
            failures.append(f"A14 did not scan a task's own script outside ci/ (R8): {a14_rows}")
        # R9: an EXECUTED waiver covers one physical line; a second copy is a row.
        (a14_root / "ci" / "x" / "exec.sh").write_text("#!/bin/bash\nuv lock\ntrue\nuv lock\n")
        a14_exec = {("ci/x/exec.sh", "uv lock"): "EXECUTED: test"}
        a14_rows = a14_run(a14_proj(a14_good + " && bash ci/x/exec.sh"), allow=a14_exec)
        if not any("is EXECUTED but matches 2 lines" in r for r in a14_rows):
            failures.append(f"A14 did not red a second copy of an EXECUTED waiver line (R9): {a14_rows}")
        (a14_root / "ci" / "x" / "exec1.sh").write_text("#!/bin/bash\nuv lock\n")
        if a14_run(a14_proj(a14_good + " && bash ci/x/exec1.sh"), allow={("ci/x/exec1.sh", "uv lock"): "EXECUTED: test"}):
            failures.append("A14 reddened a single EXECUTED waiver line (R9)")
        if a14_run(a14_proj(a14_good + " && bash ci/x/exec.sh"), allow={("ci/x/exec.sh", "uv lock"): "PROSE: test"}):
            failures.append("A14 reddened two copies of a PROSE waiver line (R9 applies to EXECUTED only)")
        # R7: uv configuration keys that make a plain `uv run` change the environment.
        for a14_cfg_path, a14_cfg, a14_want in (
            ("py/pyproject.toml", "[tool.uv]\nreinstall = true\n", True),
            ("py/pyproject.toml", "[tool.uv]\ncache-keys = [{ file = 'x' }]\n", True),
            ("py/pyproject.toml", "[tool.uv.pip]\nupgrade = true\n", True),
            ("ci/x/uv.toml", "upgrade-package = ['x']\n", True),
            ("ci/x/uv.toml", "[pip]\nreinstall-package = ['x']\n", True),
            ("py/pyproject.toml", "[tool.uv]\nconstraint-dependencies = ['x>1']\n", False),
            ("py/pyproject.toml", "[project]\nname = 'x'\n# reinstall = true\n", False),
            ("ci/x/uv.toml", "[tool.uv]\nreinstall = true\n", False),
        ):
            if bool(uv_config_rows(a14_cfg_path, a14_cfg)) is not a14_want:
                failures.append(f"A14 R7 reads {a14_cfg_path} {a14_cfg!r} as {'green' if a14_want else 'red'}")
        try:
            uv_config_rows("py/pyproject.toml", "[tool.uv\n")
            failures.append("A14 R7 did not raise on a pyproject.toml that is not valid TOML")
        except MoonOutputError:
            pass
        (a14_root / "py" / "pyproject.toml").write_text("[tool.uv]\nupgrade = true\n")
        a14_rows = a14_run(a14_proj(a14_good), uv_configs=["py/pyproject.toml"])
        if not any("py/pyproject.toml" in r and "upgrade" in r for r in a14_rows):
            failures.append(f"A14 did not red a mutating key in a scanned pyproject.toml (R7): {a14_rows}")
        a14_rows = a14_run(a14_proj(a14_good), uv_configs=[])
        if not any(r.startswith("FLOOR:") and "py/pyproject.toml" in r for r in a14_rows):
            failures.append(f"A14 gave no FLOOR row for a uv configuration list without py/pyproject.toml: {a14_rows}")
        (a14_root / "py" / "pyproject.toml").write_text("[tool.uv]\n")
        a14_rows = a14_run(a14_proj(a14_good, smoke_globs=["ci/**/*"]), uv_configs=["py/pyproject.toml"])
        if not any("py/pyproject.toml" in r and "matches no input" in r for r in a14_rows):
            failures.append(f"A14 G5 did not cover a scanned uv configuration file: {a14_rows}")
        # R5 / R-toolchain: an argument that Moon appends to its own `uv sync`.
        for a14_tc, a14_want in (
            ("unstable_uv:\n  version: '0.11.16'\n  syncArgs: ['--reinstall']\n", True),
            ("unstable_python:\n  venvArgs:\n    - '--clear'\n", True),
            ("unstable_uv:\n  # syncArgs: ['--reinstall']\n  version: '0.11.16'\n", False),
            ("unstable_uv: {version: '0.11.16', syncArgs: ['--reinstall']}\n", True),
            ("unstable_uv:\n  \"syncArgs\": ['--reinstall']\n", True),
            ("unstable_uv:\n  version: '0.11.16' # syncArgs: x\n", False),
            ("{$schema: ' # local', unstable_uv: {syncArgs: ['--reinstall']}}\n", True),
            ('{$schema: " # local", unstable_uv: {syncArgs: [x]}}\n', True),
            ("{a: 'it''s # x', unstable_uv: {syncArgs: [x]}}\n", True),
            ('{a: "x\\" # y", unstable_uv: {syncArgs: [x]}}\n', True),
        ):
            if bool(moon_toolchain_rows(".moon/toolchains.yml", a14_tc)) is not a14_want:
                failures.append(f"A14 toolchain rule reads {a14_tc!r} as {'green' if a14_want else 'red'}")
        (a14_root / ".moon").mkdir()
        (a14_root / ".moon" / "toolchains.yml").write_text("unstable_uv:\n  syncArgs: ['--reinstall']\n")
        a14_rows = a14_run(a14_proj(a14_good, smoke_globs=[*a14_smoke_globs, ".moon/**/*"]), moon_toolchains=".moon/toolchains.yml")
        if not any(".moon/toolchains.yml" in r and "syncArgs" in r for r in a14_rows):
            failures.append(f"A14 did not red a syncArgs entry in the Moon toolchain file: {a14_rows}")
        a14_rows = a14_run(a14_proj(a14_good), moon_toolchains=".moon/missing.yml")
        if not any(r.startswith("FLOOR:") and "missing.yml" in r for r in a14_rows):
            failures.append(f"A14 gave no FLOOR row for a missing Moon toolchain file: {a14_rows}")
        # The floor gives no credit to an isolated run that sits in a multi-line quote or after a `#` word.
        a14_iso = "uv run --isolated --reinstall-package paigasus-py-bindings pytest tests"
        for a14_blob in (f"echo '\n{a14_iso}\n'\nuv run pytest tests",
                         f"echo ok # ; {a14_iso}\nuv run pytest tests"):
            if not any(r.startswith("FLOOR:") for r in a14_run(a14_proj(a14_blob))):
                failures.append(f"A14 gave floor credit to a quoted or commented isolated run: {a14_blob!r}")
        # The live waiver table: every reason opens with a class word, and exactly one entry is EXECUTED.
        # (The live reasons read "PROSE, not an invocation ...", so the class word ends at a word boundary.)
        if not all(re.match(r"(?:PROSE|DATA|EXECUTED)\b", a14_r) for a14_r in ALLOW_UV_ENV_SCRIPT.values()):
            failures.append("ALLOW_UV_ENV_SCRIPT has a reason that does not start with PROSE, DATA or EXECUTED")
        if sum(a14_r.startswith("EXECUTED") for a14_r in ALLOW_UV_ENV_SCRIPT.values()) != 1:
            failures.append("ALLOW_UV_ENV_SCRIPT must have exactly one EXECUTED entry")
        a14_stale = {("ci/x/msg.sh", "echo 'gone'"): "stale on purpose"}
        if not any("stale" in r for r in a14_run(a14_proj(a14_good), allow=a14_stale)):
            failures.append("A14 did not red a stale allowlist entry")
        try:
            a14_run(a14_proj(a14_good + " && bash scripts/missing.sh"))
            failures.append("A14 did not raise on an unreadable project script")
        except MoonOutputError:
            pass
        # A leading `./` must not hide the script (fail-open before SMA-765's final review).
        a14_rows = a14_run(a14_proj(a14_good + " && bash ./scripts/run_tests.sh"))
        if not any("py/scripts/run_tests.sh" in r for r in a14_rows):
            failures.append(f"A14 did not red a `uv pip install` in `bash ./scripts/...`: {a14_rows}")
        # `../scripts/x.sh` resolves against the project dir: from `py/sub` it is `py/scripts/x.sh`.
        a14_rows = a14_run(a14_proj(a14_good + " && bash ../scripts/run_tests.sh", source_dir="py/sub"))
        if not any("py/scripts/run_tests.sh" in r for r in a14_rows):
            failures.append(f"A14 did not red a `uv pip install` in `bash ../scripts/...`: {a14_rows}")
        # A project script follows `source` like a ci/ script does (SMA-765 round 2).
        (a14_root / "py" / "scripts" / "mutate.sh").write_text("#!/bin/bash\nuv sync\n")
        (a14_root / "py" / "scripts" / "wrap.sh").write_text("#!/bin/bash\nsource ./mutate.sh\n")
        (a14_root / "py" / "scripts" / "bad.sh").write_text("#!/bin/bash\nsource ./gone.sh\n")
        a14_rows = a14_run(a14_proj(a14_good + " && bash scripts/wrap.sh"))
        if not any("py/scripts/mutate.sh" in r for r in a14_rows):
            failures.append(f"A14 did not red a `uv sync` in a script sourced by a project script: {a14_rows}")
        try:
            a14_run(a14_proj(a14_good + " && bash scripts/bad.sh"))
            failures.append("A14 did not raise on an unresolvable path sourced by a project script")
        except MoonOutputError:
            pass
        (a14_root / "py" / "scripts" / "semi.sh").write_text("#!/bin/bash\ntrue; source ./mutate.sh\n")
        (a14_root / "py" / "scripts" / "and.sh").write_text("#!/bin/bash\nx && . ./mutate.sh\n")
        (a14_root / "py" / "scripts" / "then.sh").write_text("#!/bin/bash\nif x; then source ./mutate.sh; fi\n")
        (a14_root / "py" / "scripts" / "badsemi.sh").write_text("#!/bin/bash\ntrue; source ./gone.sh\n")
        for a14_s in ("semi", "and", "then"):
            a14_rows = a14_run(a14_proj(a14_good + f" && bash scripts/{a14_s}.sh"))
            if not any("py/scripts/mutate.sh" in r for r in a14_rows):
                failures.append(f"A14 did not follow a mid-line source in {a14_s}.sh: {a14_rows}")
        try:
            a14_run(a14_proj(a14_good + " && bash scripts/badsemi.sh"))
            failures.append("A14 did not raise on an unresolvable mid-line sourced path")
        except MoonOutputError:
            pass
        (a14_root / "py" / "scripts" / "brace.sh").write_text("#!/bin/bash\n{ source ./mutate.sh; }\n")
        a14_rows = a14_run(a14_proj(a14_good + " && bash scripts/brace.sh"))
        if not any("py/scripts/mutate.sh" in r for r in a14_rows):
            failures.append(f"A14 did not follow `{{ source x; }}`: {a14_rows}")
        (a14_root / "ci" / "x" / "mutate.sh").write_text("#!/bin/bash\nuv sync\n")
        (a14_root / "ci" / "x" / "semi.sh").write_text("#!/bin/bash\ntrue; source ./mutate.sh\n")
        a14_rows = a14_run(a14_proj(a14_good + " && bash ci/x/semi.sh"))
        if not any("ci/x/mutate.sh" in r for r in a14_rows):
            failures.append(f"A14 did not follow a mid-line source in a ci/ script: {a14_rows}")
        for a14_split in (
            "uv run --isolated --reinstall-package paigasus-py-bindings true && uv run pytest tests",
            "uv run --isolated --reinstall-package paigasus-py-bindings true",
        ):
            if not any(r.startswith("FLOOR:") for r in a14_run(a14_proj(a14_split))):
                failures.append(f"A14 gave no FLOOR row for an isolated run that does not run pytest: {a14_split!r}")
        a14_rows = a14_run(a14_proj("uv run --isolated --reinstall-package paigasus-py-bindings python -m pytest tests"))
        if any(r.startswith("FLOOR:") for r in a14_rows):
            failures.append(f"A14 gave a FLOOR row for `python -m pytest`: {a14_rows}")
        (a14_root / "py" / "scripts" / "asrc.sh").write_text("#!/bin/bash\nsource as arg\n")
        (a14_root / "py" / "scripts" / "as").write_text("#!/bin/bash\nuv sync\n")
        a14_rows = a14_run(a14_proj(a14_good + " && bash scripts/asrc.sh"))
        if not any("py/scripts/as:" in r for r in a14_rows):
            failures.append(f"A14 did not follow `source as arg`: {a14_rows}")
        (a14_root / "py" / "scripts" / "as").unlink()
        try:
            a14_run(a14_proj(a14_good + " && bash scripts/asrc.sh"))
            failures.append("A14 did not raise on `source as` with no such file")
        except MoonOutputError:
            pass
        (a14_root / "py" / "scripts" / "jq.sh").write_text("#!/bin/bash\njq 'else . as $rows | $rows'\n")
        if a14_run(a14_proj(a14_good + " && bash scripts/jq.sh")):
            failures.append("A14 followed the jq binding `. as $rows` as a source")
        a14_quoted = "echo 'uv run --isolated --reinstall-package paigasus-py-bindings pytest tests'\nuv run pytest tests"
        if not any(r.startswith("FLOOR:") for r in a14_run(a14_proj(a14_quoted))):
            failures.append("A14 credited the floor to a `uv run` inside quotes")
        a14_quoted2 = "echo 'x && uv run --isolated --reinstall-package paigasus-py-bindings pytest tests'\nuv run pytest tests"
        if not any(r.startswith("FLOOR:") for r in a14_run(a14_proj(a14_quoted2))):
            failures.append("A14 credited the floor to a `uv run` after `&&` inside quotes")
        a14_cmt = "echo ok # \\\necho '\nuv run --isolated --reinstall-package paigasus-py-bindings pytest tests\n'\nuv run pytest tests"
        if not any(r.startswith("FLOOR:") for r in a14_run(a14_proj(a14_cmt))):
            failures.append("A14 credited the floor to a `uv run` inside a quote that follows a comment ending in a backslash")
        a14_cmt2 = "echo ok # \\\nuv run --isolated --reinstall-package paigasus-py-bindings pytest tests"
        if any(r.startswith("FLOOR:") for r in a14_run(a14_proj(a14_cmt2))):
            failures.append("A14 gave no floor credit to a real command on the line after a comment that ends in a backslash")
        a14_iso = "uv run --isolated --reinstall-package paigasus-py-bindings pytest tests"
        a14_esc = "echo a\\ # '\n" + a14_iso + "\n'\nuv run pytest tests"
        if not any(r.startswith("FLOOR:") for r in a14_run(a14_proj(a14_esc))):
            failures.append("A14 treated a `#` after an escaped space as a comment and credited a quoted `uv run`")
        a14_escq = "echo a\\'b\n" + a14_iso
        if any(r.startswith("FLOOR:") for r in a14_run(a14_proj(a14_escq))):
            failures.append("A14 let an escaped quote open a quote and gave no floor credit to the next line")
        a14_dq = "echo \"a\\\"\" # '\n" + a14_iso
        if any(r.startswith("FLOOR:") for r in a14_run(a14_proj(a14_dq))):
            failures.append("A14 let a `'` in a comment after a closed double quote open a quote")
        # A `../` ref that resolves to no file must raise, not be skipped.
        try:
            a14_run(a14_proj(a14_good + " && bash ../scripts/missing.sh", source_dir="py/sub"))
            failures.append("A14 did not raise on an unresolvable `../scripts/` reference")
        except MoonOutputError:
            pass

    # SMA-768 follow-up: every file read goes through `_read_utf8`. A file that is not UTF-8 is
    # MoonOutputError (rc 2) that names the file, never a raw UnicodeDecodeError (rc 1). A13 keeps
    # its own contract: an unreadable file is a row that names it.
    rd_bad = bytes([0x23, 0x20, 0xFF, 0x0A])

    def rd_infra(label, name, fn):
        try:
            fn()
        except MoonOutputError as exc:
            if "not valid UTF-8" not in str(exc) or name not in str(exc):
                failures.append(f"UTF-8 read, {label}: the error does not name {name}: {exc}")
            return
        except Exception as exc:
            failures.append(f"UTF-8 read, {label}: raised {type(exc).__name__}, not MoonOutputError: {exc}")
            return
        failures.append(f"UTF-8 read, {label}: a file that is not UTF-8 raised nothing")

    with tempfile.TemporaryDirectory() as rd_tmp:
        rd_root = Path(rd_tmp)
        (rd_root / "a8.sh").write_bytes(rd_bad)
        rd_infra("A8 script_cargo_lines", "a8.sh", lambda: script_cargo_lines(rd_root / "a8.sh"))
        (rd_root / "rs" / "crates" / "libs" / "x").mkdir(parents=True)
        (rd_root / "rs" / "Dockerfile").write_bytes(rd_bad)
        rd_infra("A8 check_dockerfile_locked", "Dockerfile", lambda: check_dockerfile_locked(rd_root))
        (rd_root / "rs" / "Cargo.toml").write_bytes(rd_bad)
        rd_infra("read_workspace_members", "Cargo.toml", lambda: read_workspace_members(rd_root, "A9"))
        (rd_root / "rs" / "crates" / "libs" / "x" / "Cargo.toml").write_bytes(rd_bad)
        rd_infra("cargo_crates", "Cargo.toml", lambda: cargo_crates(rd_root))
        (rd_root / "ci" / "y").mkdir(parents=True)
        (rd_root / "ci" / "y" / "run.sh").write_bytes(b"cd rs\n\xff\n")
        rd_a10 = {"repo": {"source_dir": ".", "deps": {}, "tasks": {},
                           "task_inputs": {"g": []}, "task_input_globs": {"g": []},
                           "invocations": {"g": "bash ci/y/run.sh"}}}
        rd_infra("A10 followed script", "run.sh",
                 lambda: check_cargo_config_inputs(rd_a10, rd_root, allow={}, floor=()))
        (rd_root / "pkg").mkdir()
        (rd_root / "pkg" / "package.json").write_bytes(rd_bad)
        (rd_root / "pkg" / "tsconfig.json").write_bytes(rd_bad)
        for rd_label, rd_fn in (("A13 package.json", _read_package_json), ("A13 tsconfig chain", _tsconfig_chain)):
            rd_rows = []
            try:
                rd_fn(rd_root, "pkg", rd_rows)
            except Exception as exc:
                failures.append(f"UTF-8 read, {rd_label}: raised {type(exc).__name__} in place of a row: {exc}")
                continue
            if not any("pkg/" in r and "not valid UTF-8" in r for r in rd_rows):
                failures.append(f"UTF-8 read, {rd_label}: no row names the file that is not UTF-8: {rd_rows}")
    # The pin: `_read_utf8` holds the only `read_text` call, and nothing opens a file another way.
    rd_src = _read_utf8(__file__)
    if rd_src.count("." + "read_text(") != 1 or re.search(r"\bopen\(|\.read_bytes\(|tomllib\.load\(", rd_src):
        failures.append("a file read in cargo_moon_parity.py does not go through _read_utf8")

    # SMA-768 follow-up: A13 and A14 G5 share `smoke_unreached`.
    su_proj = {AFFECTED_SMOKE_PROJECT: {"task_inputs": {AFFECTED_SMOKE_TASK: ["x/lit.sh"]},
                                        "task_input_globs": {AFFECTED_SMOKE_TASK: ["ci/**/*", "!ci/no.sh"]}}}
    su_got = smoke_unreached(su_proj, {"ci/a.sh", "ci/no.sh", "x/lit.sh", "y/b.sh"})
    if su_got != ["ci/no.sh", "y/b.sh"]:
        failures.append(f"smoke_unreached returned {su_got}, not ['ci/no.sh', 'y/b.sh']")
    if smoke_unreached({}, {"ci/a.sh"}) is not None:
        failures.append("smoke_unreached did not return None when repo:affected-smoke reports no inputGlobs")

    # SMA-767 follow-up: a crate-local `.cargo/config.toml` is demanded by A10 and by A5.
    with tempfile.TemporaryDirectory() as cl_tmp:
        cl_root = Path(cl_tmp)
        cl_rel = "rs/crates/bindings/paigasus-py-bindings/.cargo/config.toml"
        (cl_root / cl_rel).parent.mkdir(parents=True)
        (cl_root / cl_rel).write_text("[target.x86_64-apple-darwin]\n")
        cl_crate = {"pb-rs": {"source_dir": "rs/crates/bindings/paigasus-py-bindings", "deps": {}, "tasks": {},
                              "task_inputs": {"build": [CARGO_CONFIG_INPUT, cl_rel]},
                              "task_input_globs": {"build": []},
                              "invocations": {"build": "cargo build --locked"}}}
        if check_cargo_config_inputs(cl_crate, cl_root, allow={}, floor=(), local_floor=(cl_rel,)):
            failures.append("A10 reported rows on a crate task that keys on its crate-local cargo config")
        cl_broken = json.loads(json.dumps(cl_crate))
        cl_broken["pb-rs"]["task_inputs"]["build"] = [CARGO_CONFIG_INPUT]
        if not any("pb-rs:build" in r and cl_rel in r
                   for r in check_cargo_config_inputs(cl_broken, cl_root, allow={}, floor=())):
            failures.append("A10 did not red a crate task that omits its crate-local cargo config")
        cl_other = json.loads(json.dumps(cl_crate))
        cl_other["pb-rs"]["source_dir"] = "rs/crates/libs/other"
        if not any(r.startswith("FLOOR:") and cl_rel in r
                   for r in check_cargo_config_inputs(cl_other, cl_root, allow={}, floor=(), local_floor=(cl_rel,))):
            failures.append("A10 gave no FLOOR row when no task demands the crate-local cargo config")
        cl_ffi = json.loads(json.dumps(ffi_ok))
        if not any("paigasus-kernel-py:test" in r and cl_rel in r for r in check_ffi_inputs(cl_ffi, root=cl_root)):
            failures.append("A5 did not red the py FFI task that omits the crate-local cargo config")
        cl_ffi["paigasus-kernel-py"]["task_inputs"]["test"].append(cl_rel)
        if check_ffi_inputs(cl_ffi, root=cl_root):
            failures.append(f"A5 reported rows on a py FFI task that keys on the crate-local cargo config: "
                            f"{check_ffi_inputs(cl_ffi, root=cl_root)}")
        cl_ffi["paigasus-kernel-py"]["invocations"]["test"] = "uv run --isolated --reinstall-package other pytest"
        if not any(r.startswith("FLOOR:") and cl_rel in r for r in check_ffi_inputs(cl_ffi, root=cl_root)):
            failures.append("A5 gave no FLOOR row when no FFI task names the crate with a crate-local cargo config")
    for f in failures:
        print(f"  FAIL {f}", file=sys.stderr)
    if failures:
        print("negative-control FAILED: the parity gate can pass vacuously", file=sys.stderr)
        return 1
    print("  OK   [parity] all fourteen assertions fire on synthetic violations")
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
EXPECTED_FINDING_KEYS = ("a1", "a2", "a3", "a4-lint", "a4-fmt", "a5", "a6", "a7", "a8", "a9", "a10", "a11", "a12a", "a12b", "a13", "a14")


def collect_findings(projects, crates, root, tracked):
    """Every assertion's rows, as `(key, rows, title)`, in report order.

    ONE list, used for BOTH the pass/fail verdict and the report. Before SMA-542 the two were
    written separately, so a new check folded into one and not the other was a green no-op.
    That restructure is necessary but NOT sufficient on its own: what makes the list itself
    hard to shrink is `EXPECTED_FINDING_KEYS` above, asserted by `self_test`.

    Raises the INFRA_ERRORS members its checks raise (`MoonOutputError` from the FFI derivation),
    so `main` keeps them inside its try and maps them to rc 2.

    `tracked` is the set of git-tracked paths that A13 needs (SMA-736 §4.5). `main` builds it once
    with `task_inputs.tracked_files`; the self-test passes a fixed set.
    """
    a1, a2, a3 = check(projects, crates)
    a5 = check_ffi_inputs(projects, root=root)
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
             "    to that task's `inputs`, and the crate-local `.cargo/config.toml` of each binding\n"
             "    crate that the task names (SMA-767 follow-up). A `not matched by any FFI marker` row\n"
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
             "    Fix: add the missing crate directory as a literal `members` entry, with\n"
             "    no glob (SMA-663). Dependabot lists a single directory level below the\n"
             "    glob's literal prefix.\n"
             "    An `A9 examines` row means the opposite — cargo_crates() found no crates, so\n"
             "    the comparison covers nothing; fix that first."),
        ("a10", check_cargo_config_inputs(projects, root),
             "A task runs a COMPILING cargo command with cwd inside rs/ but does not key on\n"
             "    rs/.cargo/config.toml, so a rustflags edit replays its cached result\n"
             "    (SMA-594, SMA-599).\n"
             "    Fix: for a crate task the input is declared once for ALL crates in\n"
             "    .moon/tasks/rust.yml — restore it there, not per-crate. For a repo:* gate it\n"
             "    is declared in that task's own `inputs` in moon.yml. A crate-local\n"
             "    `.cargo/config.toml` row: add `.cargo/config.toml` to the `inputs` of that\n"
             "    task in the crate's own moon.yml (SMA-767 follow-up).\n"
             "    `cargo fmt`, `cargo tree`, `cargo metadata`, `cargo deny` and `cargo machete`\n"
             "    are out of scope BY VERB (they never compile or link) — see\n"
             "    CONFIG_SENSITIVE_VERBS. A `FLOOR:` row means the check itself cannot be\n"
             "    trusted; fix that first, every other A10 row is meaningless until it passes."),
        ("a11", check_member_literals(root),
             "A `[workspace] members` entry in rs/Cargo.toml carries a glob character. A glob\n"
             "    can match a dot-directory. `napi build` stages its output in\n"
             "    `.<crate>.napi-stage-<random>` beside the crate, with no Cargo.toml in it.\n"
             "    Every concurrent `cargo metadata` then fails with exit 101 (SMA-663).\n"
             "    Fix: list each crate directory as its own literal entry. A9 reds if one is\n"
             "    missing."),
        ("a12a", check_ts_tsc_inputs(projects, root),
             "A ts task that runs `tsc` does not key on a file `tsc` reads through its package.json\n"
             "    dependency closure, so a change there SELECTS NOTHING for that task and Moon\n"
             "    serves a cached PASS (SMA-536).\n"
             "    Fix: add the missing entry to that task's `inputs` in its own moon.yml. For each\n"
             "    workspace package in the closure: `/<dir>/src/**/*`, `/<dir>/package.json` and each\n"
             "    `exports` target outside src/. For each `file:` binding: `/<dir>/package.json` and\n"
             "    each `.d.ts` in its `files`. Always `/ts/tsconfig.base.json`, and\n"
             "    `/ts/scripts/check-installed-bindings.mjs` when the closure holds a binding. A\n"
             "    `deps omit contracts:generate` row needs `deps: ['contracts:generate']` on that task.\n"
             "    Extra inputs are ALLOWED (this is containment, like A7).\n"
             "    A `FLOOR:` row, or a row about an unreadable package.json, means the check itself\n"
             "    cannot be trusted — fix that first."),
        ("a12b", check_ts_tsc_preflight(projects, root),
             "A ts `tsc` task whose package closure holds a `file:` binding does not run the\n"
             "    installed-typings preflight before `tsc`, so `tsc` can read stale typings through\n"
             "    ts/node_modules and pass (SMA-536).\n"
             "    Fix: make the task a script that starts\n"
             "    `node ../../../ts/scripts/check-installed-bindings.mjs && pnpm exec tsc ...`.\n"
             "    A `FLOOR:` row means A12b examines less than the A12 floor — fix that first."),
        ("a13", check_ts_vitest_inputs(projects, root, tracked),
             "A ts task that runs vitest does not key on a file vitest reads, so a change there\n"
             "    SELECTS NOTHING for that task and Moon serves a cached PASS (SMA-736).\n"
             "    Extra inputs are ALLOWED (this is containment, like A12a). A `FLOOR:` row, or a row\n"
             "    about a config, a tsconfig or a package.json that A13 cannot read, means the check\n"
             "    itself cannot be trusted — fix that first.\n"
             "    Fix: add the missing entry to that task's `inputs` in its own moon.yml, or to the\n"
             "    inherited `test` task in .moon/tasks/typescript-project.yml: `/ts/pnpm-lock.yaml`;\n"
             "    each vitest config; per workspace package in the closure, `/<dir>/src/**/*`,\n"
             "    `/<dir>/package.json` and each `exports` target outside src/; per `file:` binding,\n"
             "    `/<dir>/package.json` and each `files` entry that is not a `.d.ts`; each\n"
             "    tsconfig.json with its `extends` chain, unless every config sets `tsconfig: false`;\n"
             "    each tracked alias target outside the own package. A `deps omit contracts:generate`\n"
             "    row needs `deps: ['contracts:generate']` on that task. For a \"matches no input of\n"
             "    repo:affected-smoke\" row, add a glob that matches the config to the inputs of\n"
             "    `repo:affected-smoke` in the root moon.yml. See ci/affected-graph/README.md (A13)."),
        ("a14", check_uv_env_isolation(
            projects, root,
            uv_configs=uv_configs_of(tracked) if tracked else None,
            moon_toolchains=MOON_TOOLCHAINS if tracked else None,
        ),
             "A task can change a uv environment IN PLACE that another task reads, so two parallel\n"
             "    tasks race on py/.venv and one loses packages it imports (SMA-765, SMA-768).\n"
             "    Fix: use an approved uv form, or add the form to the allow-list constants in\n"
             "    cargo_moon_parity.py with a self-test row. Use `uv run --isolated` (with\n"
             "    `--reinstall-package paigasus-py-bindings` for the kernel task). Do not run `uv sync`,\n"
             "    `uv pip`, `uv venv`, `uv add` or `uv remove` in a Moon task. Do not name UV_*,\n"
             "    VIRTUAL_ENV or --active. A script line that only prints `uv` needs an\n"
             "    ALLOW_UV_ENV_SCRIPT entry with a reason that starts with PROSE, DATA or EXECUTED.\n"
             "    For a \"matches no input of repo:affected-smoke\" row, add a glob for the file to the\n"
             "    inputs of `repo:affected-smoke` in moon.yml and to T_AFFECTED_SMOKE_REQUIRED_INPUTS\n"
             "    in ci/actionlint/run.sh.\n"
             "    A `FLOOR:` row means the check itself cannot be trusted - the floor task is missing or\n"
             "    lost `--isolated` or the bindings reinstall; fix that first. See\n"
             "    ci/affected-graph/README.md (A14)."),
    ]

    return findings


def main():
    root = Path(__file__).resolve().parents[2]
    try:
        projects = moon_projects()
        crates = cargo_crates(root)
        tracked = task_inputs.tracked_files(root)
        findings = collect_findings(projects, crates, root, tracked)
    except INFRA_ERRORS as exc:
        # Mirror run.sh's infra-vs-assertion split: a broken `moon` — or an unparseable Cargo.toml —
        # must never be mistaken for a graph regression. See INFRA_ERRORS.
        print(f"FATAL [parity] could not build the graphs: {exc}", file=sys.stderr)
        return 2

    # SMA-765: A14's real-tree negative control. Remove `--isolated` from the kernel task in a deep
    # copy of the real graph. A14 must report a row that names the kernel task and is not a
    # `FLOOR:` row, or the check cannot see the defect it exists for. The FLOOR row alone would
    # pass even if the per-task scan were dead.
    # `run.sh --negative-control` runs only the synthetic self-test, so this is an explicit step.
    try:
        mutated = json.loads(json.dumps(projects))
        for a14_target in REQUIRED_UV_ISOLATED_TASKS:
            a14_pid, _, a14_name = a14_target.partition(":")
            a14_inv = (mutated.get(a14_pid) or {}).get("invocations") or {}
            if a14_inv.get(a14_name):
                a14_inv[a14_name] = a14_inv[a14_name].replace("--isolated", "")
        a14_control = check_uv_env_isolation(mutated, root)
    except INFRA_ERRORS as exc:
        print(f"FATAL [parity] A14 negative control could not run: {exc}", file=sys.stderr)
        return 2
    if not any(
        r.startswith("paigasus-kernel-py:test") and not r.startswith("FLOOR:") for r in a14_control
    ):
        print(
            "FATAL [parity] A14 negative control: A14 stayed green after `--isolated` was removed "
            "from the kernel task, so A14 cannot see the defect it exists for",
            file=sys.stderr,
        )
        return 2

    if not any(rows for _, rows, _ in findings):
        print(
            f"PASS  {'cargo-moon-parity':<18} -> "
            f"{len(crates)} crates: every Cargo dep has a Moon edge that schedules its build, "
            f"every lint and fmt keys on the files its command reads, every FFI build task does "
            f"too, every crate keys on its upstream sources, and every py/ts wrapper keys on the "
            f"Rust crates it builds, every cargo-resolving task passes --locked, and every "
            f"workspace crate is reachable through Dependabot's member expansion, and "
            f"every compiling cargo task inside rs/ keys on .cargo/config.toml, and every "
            f"workspace members entry is a literal path, and every ts tsc task keys on its "
            f"package.json closure and runs the installed-typings preflight first when that "
            f"closure holds a binding, and every ts vitest task keys on its configs, its package.json "
            f"closure, its tsconfig chain and its alias targets, and no task changes a uv "
            f"environment in place"
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
