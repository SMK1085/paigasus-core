/**
 * @paigasus/commitlint-config — canonical Conventional Commits ruleset for Paigasus.
 * Source of truth for the type + scope allowlists; CONTRIBUTING.md mirrors these.
 */
module.exports = {
  extends: ['@commitlint/config-conventional'],
  rules: {
    'type-enum': [
      2,
      'always',
      ['feat', 'fix', 'docs', 'chore', 'refactor', 'test', 'ci', 'build', 'perf', 'style', 'revert'],
    ],
    'scope-enum': [
      2,
      'always',
      ['rs', 'py', 'ts', 'contracts', 'ci', 'docs', 'deps', 'release', 'repo', 'claude', 'workspace'],
    ],
    'scope-empty': [2, 'never'],
    'subject-empty': [2, 'never'],
    'header-max-length': [2, 'always', 100],
    'body-max-line-length': [2, 'always', 100],
    'footer-leading-blank': [2, 'always'],
  },
};
