# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

A maintainer writes this file by hand. release-plz does not process this crate, because its Cargo
manifest sets `publish = false` (SMA-658, spec § 3.1).

## [Unreleased]

## [0.2.0] - 2026-10-01

### Added

- The gateway chat surface accepts an OIDC access token from an interactive user, not only an
  API key. IAM authorizes `InvokeModel` against the user's own token and an organization scope.
  The `org_admin` role does not carry `InvokeModel`: a person needs a `gateway_user` grant to
  call the chat surface (SMA-635).
- A new request header, `paigasus-org`, names the organization for an OIDC caller. The header is
  optional when the caller reaches exactly one organization through its memberships and role
  grants: the gateway then infers that organization. The gateway ignores the header for an API
  key: an API key's scope comes from the key itself. The gateway logs one warning for each
  request that carries the header with an API key (SMA-635).
- The gateway returns two new error responses. It returns `invalid-org-header` (400) when
  `paigasus-org` is not exactly one organization UUID in the 36-character form. It returns
  `org-required` (400) when the caller sends no header and reaches zero or more than one
  organization. Both name the header `paigasus-org` in the response `param` field (SMA-635).

### Changed

- The `invalid-api-key` error message reads "Invalid credential." instead of "Invalid API key."
  The error code stays `invalid-api-key`. The message changed because this error now also covers
  a rejected user bearer, not only a rejected API key (SMA-635).

## [0.1.0] - 2026-09-20

### Added

- The first released container image: `ghcr.io/smk1085/paigasus-gateway` and
  `docker.io/smaschek/paigasus-gateway` (SMA-658).

### Added

- A new setting, `[dpop] enabled` (`GATEWAY_DPOP__ENABLED`), turns on the DPoP scheme on the
  protected routes. It is off by default. With it on, a client sends `Authorization: DPoP <token>`
  and one `DPoP` proof header. The gateway skips the API-key leg. It sends the proof, the method
  and the path to IAM, and sends the same token and proof on the self-query. Turn DPoP on in IAM
  first (SMA-700).
- With DPoP on, every 401 carries one `WWW-Authenticate: DPoP` line. A bad proof is 401
  `invalid-dpop-proof`. The DPoP quota of IAM is 429 `rate-limited` with `Retry-After`, and the
  metric label is `denied`, not `unavailable`. With DPoP off, no response changes (SMA-700).
