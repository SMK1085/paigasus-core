# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

A maintainer writes this file by hand. The version source is the `version` field of this app's
`package.json`. release-plz does not process this app (SMA-688, spec D1).

## [Unreleased]

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
