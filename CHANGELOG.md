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

- **Every published artefact now carries its licence text.** `wickra-exchange`
  and `wickra-exchange-core` are packed from their own directories, so a crates.io
  release shipped without `LICENSE-MIT`/`LICENSE-APACHE`; the six npm platform
  packages declared `MIT OR Apache-2.0` while their `files` array named only the
  `.node` binary, and nothing copied the texts into the stub directories. Copies
  now live beside each published crate and beside the Python binding, the stubs
  list both files, and `release.yml` stages them and then proves with
  `npm pack --dry-run` that npm really packs them — `files` and what npm produces
  can disagree, and that failure is silent.
- `bindings/r/LICENSE` names the same copyright holder as the rest of the family
  ("kingchenc and the Wickra contributors").
- `CITATION.cff` carries `version` and `date-released`. GitHub's citation box and
  Zenodo read both from there, and neither was present.
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
- `base64` is declared on the 0.22 line, so the workspace resolves a single copy
  of it. `wickra-exchange-core` declared `0.23` while `reqwest` pulls `0.22` in
  through `hyper-util`/`hyper-rustls`, so both were compiled into every
  artefact. The Engine API this code uses is identical across the two lines, and
  0.22.1 is also what `wickra` and `wickra-backtest` resolve.
- `cargo-deny` failed on `main`: `chacha20 0.10.1` was yanked from crates.io and
  reached the tree through `tokio-tungstenite 0.30 -> tungstenite 0.30 -> rand
  0.10.2`. Locked to `0.10.2`, which is not yanked. Nothing else moved.

### Added

- SPDX-named licence copies under `LICENSES/` (`MIT.txt`,
  `Apache-2.0.txt`) for REUSE-style tooling.
- Repository scaffolding mirrored from the `wickra-backtest` template: Cargo
  workspace, the `wickra-exchange-core` and `wickra-exchange` facade crates,
  supply-chain configuration (`deny.toml`, `osv-scanner.toml`), lint configuration
  and dual `MIT OR Apache-2.0` licensing.

[Unreleased]: https://github.com/wickra-lib/wickra-exchange/commits/main
