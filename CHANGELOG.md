# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- `cargo-deny` failed on `main`: `chacha20 0.10.1` was yanked from crates.io and
  reached the tree through `tokio-tungstenite 0.30 -> tungstenite 0.30 -> rand
  0.10.2`. Locked to `0.10.2`, which is not yanked. Nothing else moved.

### Added

- Repository scaffolding mirrored from the `wickra-backtest` template: Cargo
  workspace, the `wickra-exchange-core` and `wickra-exchange` facade crates,
  supply-chain configuration (`deny.toml`, `osv-scanner.toml`), lint configuration
  and dual `MIT OR Apache-2.0` licensing.

[Unreleased]: https://github.com/wickra-lib/wickra-exchange/commits/main
