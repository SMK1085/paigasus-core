# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

A maintainer writes this file by hand. The version source is the `version` field of this app's
`package.json`. release-plz does not process this app (SMA-688, spec D1).

## [Unreleased]

## [0.3.0] - 2026-11-20

### Added

- The header shows the Paigasus logo, then the product name. Each console serves a
  `favicon.ico`, also for a visitor who is not signed in (SMA-773).
- The navigation is a collapsible sidebar beside the page. It lists the pages of this console
  only, in groups. A "Navigation" button collapses it at any width. It starts open from 768
  px and collapsed on a narrow screen (SMA-783).
- The brand is a console switcher. A click opens a menu with the other consoles (SMA-783).
- The user menu button and the organization switcher show initials. The full names stay in the
  accessible names (SMA-783).

### Changed

- The dialog footer is one right-aligned row at every width. The focus ring has a contrast of 3.6:1
  in the light theme. The destructive text has a contrast of 6.1:1 in the dark theme (SMA-783).
- The error page for a quota refusal shows "Usage limit reached". Its text tells the user that the
  organization used its token budget for the period (SMA-677).

### Fixed

- The chat playground sends `stream_options.include_usage`. The gateway then charges the usage that
  the upstream reports and not an estimate (SMA-677).

## [0.2.0] - 2026-09-27

### Added

- A readiness route, `GET /gateway/readyz`. It answers 503 `{"status":"unready"}` until the auth
  runtime is built and one OIDC discovery succeeded. After that it answers 200
  `{"status":"ready"}` for the life of the process. It never waits for discovery. The proxy lets
  it through with no session cookie. The Helm chart's readiness probe uses it (SMA-705).
- Two log events. `oidc.discovery_failed` has the stage `readiness` for a discovery that the
  route started. `readiness.runtime_failed` holds only the name of the error that stopped the
  runtime build (SMA-705).

## [0.1.0] - 2026-09-26

### Added

- The first released container image: `ghcr.io/smk1085/paigasus-gateway-console` and
  `docker.io/smaschek/paigasus-gateway-console` (SMA-688).
