# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

A maintainer writes this file by hand. release-plz does not process this crate, because its Cargo
manifest sets `publish = false` (SMA-658, spec § 3.1).

## [Unreleased]

### Security

- IAM refuses a bearer token whose `typ` claim is `ID` or `Logout`. Keycloak sets these values
  on its ID token and its back-channel logout token. Before, IAM accepted such a token when its
  `aud` held the accepted audience, which is the client id in the default chart configuration.
  The refusal is logged at `info` (SMA-686).

## [0.1.0] - 2026-09-20

### Added

- The first released container image: `ghcr.io/smk1085/paigasus-iam` and
  `docker.io/smaschek/paigasus-iam` (SMA-658).
