# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

A maintainer writes this file by hand. release-plz does not process this crate, because its Cargo
manifest sets `publish = false` (SMA-658, spec § 3.1).

## [Unreleased]

## [0.2.0] - 2026-09-27

### Added

- The gateway chat surface accepts an OIDC access token from an interactive user, not only an
  API key. IAM authorizes `InvokeModel` against the user's own token and an organization scope
  (SMA-635).
- A new request header, `paigasus-org`, names the organization for an OIDC caller. The header is
  optional when the user belongs to exactly one organization: the gateway then infers that
  organization. The header is ignored, with one warning, for an API key: an API key's scope
  comes from the key itself (SMA-635).
- Two new error responses. `invalid-org-header` (400) fires when `paigasus-org` is not exactly
  one organization UUID in the 36-character form. `org-required` (400) fires when the caller
  sends no header and reaches zero or more than one organization. Both name the header
  `paigasus-org` in the response `param` field (SMA-635).

### Changed

- The `invalid-api-key` error message reads "Invalid credential." instead of "Invalid API key."
  The error code stays `invalid-api-key`. The message changed because this error now also covers
  a rejected user bearer, not only a rejected API key (SMA-635).

## [0.1.0] - 2026-09-20

### Added

- The first released container image: `ghcr.io/smk1085/paigasus-gateway` and
  `docker.io/smaschek/paigasus-gateway` (SMA-658).
