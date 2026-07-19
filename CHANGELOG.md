# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Document the supported platforms, command lifecycle, worker usage, minimum
  Rust version, and security reporting process.
- Improve built-in command help and refresh JavaScript example dependencies.

### Security

- Remove known vulnerable versions from the checked-in JavaScript example
  lockfiles and add monthly Dependabot update configuration.
- Replace the unmaintained `rustls-pemfile` dependency with the maintained PEM
  parser in `rustls-pki-types`.

## [0.1.3](https://github.com/ineentho/lazy/compare/v0.1.2...v0.1.3) - 2026-07-19

### Fixed

- *(release)* allow git-only package releases

## [0.1.2](https://github.com/ineentho/lazy/compare/v0.1.1...v0.1.2) - 2026-07-19

### Fixed

- *(proxy)* harden network and control socket boundaries ([#10](https://github.com/ineentho/lazy/pull/10))
- *(runner)* track child exits and release process resources ([#8](https://github.com/ineentho/lazy/pull/8))

### Other

- *(release)* automate release PRs and binary workflow dispatch ([#11](https://github.com/ineentho/lazy/pull/11))
- *(worker)* remove unused while configuration ([#9](https://github.com/ineentho/lazy/pull/9))
- *(release)* add licensing and package metadata ([#7](https://github.com/ineentho/lazy/pull/7))
- remove temporary release caveat
- simplify binary installation
