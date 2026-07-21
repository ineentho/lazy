# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.9](https://github.com/ineentho/lazy/compare/v0.1.8...v0.1.9) - 2026-07-21

### Added

- *(routing)* support prefixes before xip service names ([#29](https://github.com/ineentho/lazy/pull/29))

### Added

- Support variable prefixes before registered service names in xip hostnames.

## [0.1.8](https://github.com/ineentho/lazy/compare/v0.1.7...v0.1.8) - 2026-07-21

### Added

- *(proxy)* serve live status page on bare routing host ([#26](https://github.com/ineentho/lazy/pull/26))

### Other

- Add stop controls to status page
- Support IPv6 loopback upstreams

## [0.1.7](https://github.com/ineentho/lazy/compare/v0.1.6...v0.1.7) - 2026-07-20

### Added

- *(proxy)* support privileged ports via socket activation ([#24](https://github.com/ineentho/lazy/pull/24))

## [0.1.6](https://github.com/ineentho/lazy/compare/v0.1.5...v0.1.6) - 2026-07-20

### Fixed

- *(release)* verify tags before building release binaries

### Other

- *(readme)* streamline installation and basic usage ([#22](https://github.com/ineentho/lazy/pull/22))

## [0.1.5](https://github.com/ineentho/lazy/compare/v0.1.4...v0.1.5) - 2026-07-20

### Added

- detect frameworks in package-manager scripts ([#18](https://github.com/ineentho/lazy/pull/18))

### Fixed

- suppress proxy errors and idle runner message ([#19](https://github.com/ineentho/lazy/pull/19))
- preserve runner working directory for spawned commands

### Added

- Detect supported frameworks in simple package-manager scripts so commands
  such as `pnpm run dev` receive the allocated port and host arguments.

### Fixed

- Run HTTP services and workers in their runner's starting directory by
  default, and support `--cwd` for workers as well as HTTP services.

## [0.1.4](https://github.com/ineentho/lazy/compare/v0.1.3...v0.1.4) - 2026-07-19

### Other

- *(deps)* bump actions/checkout from 6 to 7 ([#14](https://github.com/ineentho/lazy/pull/14))
- refresh docs, security config, and example dependencies

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
