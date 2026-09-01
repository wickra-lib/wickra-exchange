# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Security

- **`release.yml` refuses to publish from anything but a version tag.** A new
  `gate` job runs before every publishing job and fails unless the ref is
  `refs/tags/v*`. `workflow_dispatch` exists so a release whose publish step
  failed can be re-run without moving the tag; dispatched from `main` it would
  previously have pushed whatever the manifests said to crates.io, PyPI, npm,
  NuGet and Maven Central, and `go-mirror` would have replaced the contents of
  the public `wickra-exchange-go` and tagged it `vmain`. This repository defines
  no `release` environment, so the `environment:` lines on the publish jobs
  carried no deployment-branch policy and nothing else stood in the way.
- The gate additionally refuses to publish a commit whose `ci.yml` run is not
  green, and whose other workflow runs are not green, waiting up to 45 minutes
  for a still-running verdict rather than treating an undecided run as a failure.
- Build-provenance attestation now covers the `.nupkg`, the `.jar` and the C ABI
  archives. Previously only the crates and the Python artefacts were attested,
  while Scorecard reported Signed-Releases green regardless — it looks for a
  provenance file on the release, not for coverage of what the release contains.

### Fixed

- `github-release` now waits for `csharp-publish`, `java-publish` and
  `go-mirror` as well, so the release page cannot be assembled before every
  artefact that belongs on it exists.
- `java-publish` uploads the built JAR as a workflow artifact, and
  `github-release` stages it — the Maven artefact was missing from the release
  page entirely.
- `go-mirror` builds, vets and smoke-runs the assembled module against the
  staged native library before pushing it. The push replaces the contents of a
  public repository; previously the first `go get` was the first build, so a
  broken cgo directive or a missing native library would only surface after the
  tag existed.

### Added

- Repository scaffolding mirrored from the `wickra-backtest` template: Cargo
  workspace, the `wickra-exchange-core` and `wickra-exchange` facade crates,
  supply-chain configuration (`deny.toml`, `osv-scanner.toml`), lint configuration
  and dual `MIT OR Apache-2.0` licensing.

[Unreleased]: https://github.com/wickra-lib/wickra-exchange/commits/main
