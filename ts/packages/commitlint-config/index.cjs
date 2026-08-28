/**
 * @paigasus/commitlint-config — canonical Conventional Commits ruleset for Paigasus.
 * Source of truth for the type + scope allowlists; CONTRIBUTING.md mirrors these.
 */

/**
 * release-plz's own release commit (SMA-579). It writes the release PR's commit message and
 * the PR title from the SAME string — measured in the pinned 0.3.158 source, where
 * `release_pr/mod.rs:421` commits with `&pr.title` — and all three of its default shapes are
 * unreachable for this ruleset:
 *
 *   chore: release v0.1.0                    several packages, one shared version -> scope-empty
 *   chore: release                           several packages, versions differ    -> scope-empty
 *   chore(paigasus-proto): release v0.2.0    exactly one package updates          -> scope-enum
 *
 * Note the third fails a DIFFERENT rule, so a pattern covering only the scopeless forms would
 * red on a later release rather than on this one.
 *
 * `ignores` turns off EVERY rule for a matching commit, not only the scope rules. The pattern
 * therefore anchors the WHOLE message rather than the header: release-plz's commit carries an
 * empty body (measured on PR 170), so any body makes the commit lint normally again and this
 * exception cannot be used to smuggle an unlinted payload past the gate.
 *
 * The third shape's scope is a CARGO PACKAGE NAME, so the pattern requires the `paigasus-`
 * prefix every member of this workspace carries. A bare `[a-z0-9-]+` there would exempt
 * `chore(evil): release v0.1.0` as well — measured, it passed — which silently widens
 * scope-enum for anyone who copies the shape.
 *
 * The alternative — a `pr_name` template in rs/release-plz.toml — was rejected deliberately.
 * It is a Tera template, Tera 1.20 hard-errors on an undefined variable, and release-plz binds
 * `version` only when every updated package shares one version. With two version groups
 * (`kernel`, `proto`) that template would break `release-plz release-pr` on the first
 * divergent release. This exception cannot break the release workflow.
 */
const RELEASE_PLZ_COMMIT = /^chore(\(paigasus-[a-z0-9-]+\))?: release( v\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?)?\s*$/;

module.exports = {
  extends: ['@commitlint/config-conventional'],
  ignores: [(message) => RELEASE_PLZ_COMMIT.test(message)],
  rules: {
    'type-enum': [2, 'always', ['feat', 'fix', 'docs', 'chore', 'refactor', 'test', 'ci', 'build', 'perf', 'style', 'revert']],
    'scope-enum': [2, 'always', ['rs', 'py', 'ts', 'contracts', 'ci', 'docs', 'deps', 'release', 'repo', 'claude', 'workspace']],
    'scope-empty': [2, 'never'],
    'subject-empty': [2, 'never'],
    'header-max-length': [2, 'always', 100],
    'body-max-line-length': [2, 'always', 100],
    'footer-leading-blank': [2, 'always'],
  },
};
