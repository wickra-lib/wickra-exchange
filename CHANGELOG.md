# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Security

- `GHSA-6w46-j5rx-g56g` (pytest tmpdir handling) is assessed and recorded in
  `osv-scanner.toml`. The attack vector is local — a second user on the same
  machine — and CI runners are single-user and ephemeral. It is also not fixable
  on the row it is reported for: the finding is in the Python 3.9 lock, and every
  pytest carrying the fix declares `requires-python >= 3.10`. Removed when the
  support matrix drops 3.9, which is the real fix.
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

- The five long-form issue templates and the detailed pull-request template,
  adapted to this domain rather than copied — venues and order execution where
  the originals ask about indicators and TA-Lib, and no WASM row, because there
  is no WASM binding. The main PR template now points at the long form: GitHub
  offers no picker for a second template, so it is reachable only by appending
  `?template=detailed.md` to the URL, and a template nothing mentions is a
  template nobody uses.
- `docs/README.md` — an index of the eight pages in `docs/`, and a note that they
  live beside the code on purpose: there is no separate docs repository here, so
  a page and the behaviour it documents cannot drift apart across two merges.
- `fuzz/README.md` and `fuzz/.gitignore` — what each of the five targets
  exercises, and why: everything a remote server can put on the wire is
  untrusted, and a panic across the C ABI is undefined behaviour.
- `.gitignore` for the C#, Java, Go and R bindings. The root file covers build
  output; these cover what the *release* stages into the tree —
  `WickraExchange/runtimes/`, `src/main/resources/native/`, `bindings/go/lib/` —
  which is where a multi-megabyte native library would otherwise be committed by
  accident.
- README sections **Testing** and **Benchmarks**, and `## Building from source`
  is now `## Building everything from source`. The Testing section ends with what
  the offline suites *cannot* tell you: every venue test feeds the client a
  response the test itself wrote, which proves the parser handles that shape and
  not that the shape is what the venue sends.
- **`examples-smoke` CI job — every example is now built or parsed.** Only
  `examples/c` was ever compiled, so the other seven could rot with nothing to
  say so, and one had: the C# example called a method the binding does not have
  and sat that way in the tree. Syntax-checking would not have caught it —
  `Balances()` is valid C#. The compiled examples are therefore compiled against
  the binding in this tree: C# via `dotnet build`, Go through a throwaway module
  with a `replace` onto `bindings/go`, Java with `javac` against the freshly
  packaged jar. Node, Python and R are parsed.
- **`python-wheel-container-smoke` CI job.** The manylinux and musllinux wheels
  were built for the first time by the release itself, which is irreversible.
  This builds both on every change *and installs and imports them under the
  matching libc* — a musllinux wheel cannot be installed on the glibc runner, so
  importing it there would prove nothing.
- `semver` CI job (`cargo-semver-checks`). Neither crate is on crates.io yet, so
  it looks the baseline up first and skips loudly until there is one; from the
  first release it starts checking on its own. Deliberately not
  `continue-on-error`, which would hide the API break it exists to catch.
- Non-blocking `links` job on pull requests. `links.yml`'s header has described
  this job since the repository was seeded — it just did not exist.
- **`scripts/check_binding_surface.py`** — reads the trait methods out of
  `crates/wickra-exchange-core/src/traits.rs` and holds all seven bindings to
  them. Each binding is written separately and tested separately, so a method
  missing from one of them failed nowhere; nothing compared the bindings to each
  other.
- `scripts/check_version_sync.py`, `check_readme_links.py`,
  `check_license_copies.py`, `check_r_abi_skew.py` and
  `scripts/update-lockfiles.sh`, all wired into a `binding-surface` CI job.
- `.github/requirements/ci-dev-py3.{in,txt}` and `ci-dev-py39.{in,txt}` —
  hash-pinned. The Python job installed `maturin pytest` unpinned, so a run could
  differ from the one before it for reasons nothing recorded.
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

- **`osv-scanner` was not scanning the CI Python lockfiles.** It discovers
  lockfiles by filename, and neither `ci-dev-py3.txt` nor `ci-dev-py39.txt`
  matches a pattern it recognises — the job's first run listed seven scanned
  files and neither was among them. They are named after the interpreter they
  target rather than after the format, which is the right name for a reader and
  the wrong one for the scanner, so the format is now stated explicitly with
  `--lockfile=requirements.txt:...` instead of renaming them. The claim in
  `dependabot.yml` that osv-scanner covers these files is now true; it was not
  when it was written.
- Dependabot no longer proposes version updates for `.github/requirements`. The
  two lockfiles there are resolved against different interpreters — 3.9 and 3.11
  — and Dependabot cannot see that: it reads two requirements files in one
  directory and bumps both to the same version. Its first two attempts each put
  pytest 9 into the 3.9 row, and pytest 9 declares `requires-python >= 3.10`, so
  `pip install --require-hashes` would fail on the bound. Regeneration stays with
  `scripts/update-lockfiles.sh`, which passes each row's target Python to `uv`.
  Security updates are exempt from the limit and still arrive, which is why there
  is deliberately no `ignore` list.
- The committed `bindings/node/index.js` and `index.d.ts` are checked against
  what `napi build` produces. Both are generated and committed so consumers get
  types without a build step, and nothing compared the pair — napi rewrites them
  only when somebody rebuilds, and a rebuild is not part of committing.
- `lycheeverse/lychee-action` carried the pin comment `# v2`, too coarse for
  Dependabot to resolve a version from — the same defect as `rust-cache`.
- **R could not connect to a live venue.** The binding had `wkex_paper` and
  `wkex_replay_trades` and no `wkex_connect`, so an R user could open the
  derivatives, advanced-orders, user-data and ws-execution handles — each of
  which connects internally — while having no way to construct a plain exchange
  for market data and order execution. Found by the first run of
  `check_binding_surface.py`; a verb check could not have found it, because a
  constructor is not a trait method.
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
