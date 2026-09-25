# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

A maintainer writes this file by hand. release-plz does not process this crate, because its Cargo
manifest sets `publish = false` (SMA-658, spec § 3.1).

## [Unreleased]

### Security

- IAM refuses a bearer token that is an ID token or a back-channel logout token. The markers are
  a `typ` claim of `ID` or `Logout` (Keycloak), a header `typ` of `logout+jwt`, or the
  back-channel logout `events` claim. Before, IAM accepted such a token when its `aud` held the
  accepted audience, which is the client id in the default chart configuration (SMA-686).
- IAM refuses a token with no `aud` as an audience mismatch. It logs a wrong or missing audience
  and a refused non-access token at `info`, with the issuer and a static or configured detail, at
  most once per issuer and kind in 10 seconds. Before, nothing logged these refusals, although
  the chart runbook said a wrong audience did (SMA-686).

## [0.1.0] - 2026-09-20

### Added

- The first released container image: `ghcr.io/smk1085/paigasus-iam` and
  `docker.io/smaschek/paigasus-iam` (SMA-658).
