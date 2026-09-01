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

### Added

- `actionlint` workflow. zizmor reads the workflows for security; actionlint
  reads them for whether they work at all — unknown contexts, invalid `needs`
  references, and, through its bundled shellcheck, every `run:` block.
- SPDX-named licence copies under `LICENSES/` (`MIT.txt`,
  `Apache-2.0.txt`) for REUSE-style tooling.
- Repository scaffolding mirrored from the `wickra-backtest` template: Cargo
  workspace, the `wickra-exchange-core` and `wickra-exchange` facade crates,
  supply-chain configuration (`deny.toml`, `osv-scanner.toml`), lint configuration
  and dual `MIT OR Apache-2.0` licensing.

### Fixed

- **The release published a crate that does not exist.** `release.yml` ran
  `cargo publish -p wickra-exchange-cli`, `cargo package -p wickra-exchange-cli`
  and copied its SBOM — for a `wkex` CLI crate the workspace does not contain and
  the roadmap does not plan. The steps come back with the crate; the header
  comment claiming "three crates" now names the two that exist.
- **The C# example did not compile.** It called `ex.Balances()`, which the C#
  binding has never had — the C ABI exposes a per-asset `Balance`, so the C#, Go,
  Java and R wrappers ask for one asset at a time. Nothing noticed because CI
  builds only `examples/c`; teaching CodeQL to build the C# example is what
  surfaced it.
- Nine shell defects in the workflows, found by the new `actionlint` job. Three
  publish steps used `A && B || C`, which is not if-then-else — `C` also runs
  when `A` succeeds and `B` fails, so a successful publish whose `grep` found
  nothing would have been reported as a failure. Two `local x=$(...)` assignments
  masked the command's exit status, and one `ls | wc -l` is now `find`.
- **`osv-scanner` runs**, and its first run found something `cargo-deny` does
  not: RUSTSEC-2026-0235 in `rkyv`, an *optional* `rust_decimal` feature this
  workspace does not enable. Cargo.lock records optional dependencies whether or
  not their feature is on, so the crate is in the lockfile while never being
  compiled — `cargo tree -i rkyv --target all` prints nothing. cargo-deny is
  silent because it resolves the real graph; OSV-Scanner reads the lockfile.
  Recorded as a waiver in `osv-scanner.toml` with that reasoning, to be revisited
  when `rust_decimal` moves the optional dependency to the 0.8 line. `osv-scanner.toml` existed and no workflow ever
  consulted it, so a waiver recorded there was load-bearing for nobody.
  `cargo-deny` covers the Rust graph only; the other six ecosystems — npm, PyPI,
  Maven, NuGet, Go modules, R — had no vulnerability scanning in CI at all. It
  runs with `--no-resolve`, so manifest resolution cannot fail on an `org.wickra`
  artefact that does not exist until a release publishes it; every lockfile is
  still scanned in full and transitively.
- **CodeQL analyses seven languages instead of three.** The matrix covered Rust,
  Python and JavaScript/TypeScript, leaving out exactly the five where a memory
  mistake is possible: the C ABI boundary, the Go binding handing slice base
  addresses to C through `unsafe.Pointer`, the C compiled into the R package, and
  the C#/Java handle lifetimes across an FFI arena. Example code is built and
  analysed too — it is what readers copy into their own programs.
- `.github/codeql/codeql-config.yml` — without a config every generated binding
  file raises findings anchored on a generator's source span.
- Eight action pins were behind the rest of the family and are now level with
  `wickra`: `codeql-action` v4.37.9, `taiki-e/install-action` v2.87.1,
  `r-lib/actions/setup-r` v2.13.0, `softprops/action-gh-release` v3.0.3 and
  `Swatinem/rust-cache` v2.9.2 — the last of which also carried the pin comment
  `# v2`, too coarse for Dependabot to resolve a version from, which is why it
  kept writing `# v2` back.
- Every workflow job declares `timeout-minutes` (18 did not), so a wedged job is
  capped rather than running into GitHub's six-hour default.
- `ci.yml` builds pull requests against `main` only, and its concurrency group is
  keyed on the workflow as well as the ref — runs on `main` are never cancelled,
  because `main` is the baseline every later comparison is made against.
- `deny.toml` sets `allow-wildcard-paths`: internal workspace crates are
  referenced by `path` without a version, which `wildcards = "deny"` would
  otherwise flag.
- `osv-scanner.toml` described itself as suppressions for `wickra-backtest`, the
  template this repository was seeded from.
- **`[workspace.lints.rust]` exists.** Only the clippy half was ever declared, so
  every crate inheriting `[lints] workspace = true` got no `unsafe_code`,
  `missing_debug_implementations`, `unreachable_pub` or `unused_must_use` rule at
  all — and `bindings/node/Cargo.toml` carried a comment describing itself as
  "relaxed from the workspace `forbid`" against a `forbid` that did not exist.
  The C binding now overrides `unsafe_code` to `allow` in its own manifest, which
  is where a C ABI belongs, and both bindings mirror the remaining three rules
  instead of only claiming to.
- 35 public types gained a `Debug` implementation, which the new lint required.
  The ten venue clients hold `Box<dyn HttpTransport>`, `Box<dyn WsTransport>` and
  a boxed clock closure, so those are hand-written: they report whether a
  connection is open rather than the transport itself, and **never** print
  credentials — only whether any are set. The opaque C-ABI and Python handles
  report their type name, which is all an opaque handle can honestly say.
- `[package.metadata.docs.rs] all-features = true` on both published crates.
  docs.rs otherwise builds them with default features and silently omits
  everything behind a feature gate.
- Dependabot no longer proposes `base64` 0.23. Its first rebase after #156
  reverted the declaration back to `"0.23"`, which reintroduces the second
  copy of the crate that #156 removed, so the constraint is recorded in
  `dependabot.yml` instead of being re-fixed every month. To be lifted when
  `reqwest` itself moves to 0.23.
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

[Unreleased]: https://github.com/wickra-lib/wickra-exchange/commits/main
