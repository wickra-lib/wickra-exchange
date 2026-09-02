# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Security

- **Two credentials were being printed by `Debug`.** The hand-written
  implementations added in #160 redacted `credentials` but not
  `binance.user_data_listen_key` or `kraken.ws_api_token` — a listen key grants
  read access to an account's private order and balance stream, and the Kraken
  token authenticates its WebSocket session. Both now report presence only, and
  the per-venue `Debug` tests assert it.
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

### Changed

- **The CodSpeed job now records the machine it measured on.** The job spent an
  evening failing every pull request with the same figure: `apply_delta` at
  453.6 ns against a 399.4 ns base, on five runs across two branches whose diffs
  had nothing in common — the second touched neither `orderbook.rs` nor the
  bench crate — while `main` went on reporting 399.4 although it already carried
  the code the first branch had "regressed" on. It was never a real regression,
  and that part is settled rather than assumed: built with `--emit=asm`, the two
  functions the benchmark executes are identical between the revisions,
  `apply_delta` modulo basic-block label numbering and `apply_level` but for the
  names of anonymous panic constants. The offset then disappeared on its own
  once `main` was measured again on a later commit, and base and head have
  agreed since. So the cause lay in the comparison rather than the source, but
  which part of it is not established — CodSpeed reported "Different runtime
  environments detected" without naming the dimension, and separately fell back
  to an unexpected base commit. The job now logs the CPU, kernel and toolchain
  of whatever machine produced a number, so a recurrence is diagnosable instead
  of re-derived from scratch. No threshold was added: one wide enough to hide a
  constant 12% offset would hide a real 12% regression too.

- `@napi-rs/cli` 3.7.4 → 3.8.6, with the regenerated `bindings/node/index.js`.
  The new CLI emits a different native loader — it chains load errors instead of
  discarding all but the last — so the committed file had to move with it. The
  drift check added alongside it caught this on its first real occasion; a
  Dependabot bump would otherwise have landed a stale loader.
- The napi configuration uses `binaryName` and `targets` instead of the
  deprecated `name` and `triples`. Both produce a byte-identical `index.js` and
  the same binary name, so nothing downstream moves.
- The `uv` pin in `scripts/update-lockfiles.sh` moves 0.12.7 -> 0.12.9. The
  script bootstraps that exact version when no `uv` is on PATH, so the pin
  decides which resolver produces the committed locks.
- `SECURITY.md` names the concrete first release, `0.1.0`, alongside the
  supported minor line. The blueprint audit reads this file as a version
  touchpoint and looks for the exact version, while `check_version_sync.py`
  deliberately looks for the minor line `0.1.x`, on the argument that a support
  statement is about a line and not a patch. Both are right, so the file now
  says both rather than either check being bent to fit the other.

### Added

- **A runnable example and a documented flow for reconciling after a
  reconnect.** `reconcile_orders` was reachable only by reading the source: the
  word "reconcile" appeared in the repository's documentation exactly once, in a
  directory listing. It is not an unwired internal responsibility — the library
  cannot reconcile alone, because it knows what the venue lists and not what the
  caller believed was open — but nothing showed a caller how to join the two
  halves it does provide. `docs/STREAMING.md` now does, and
  `examples/rust/src/reconcile_after_reconnect.rs` runs it: a replay tape can
  carry `Disconnected` / `Reconnected` exactly as a live stream emits them, so
  the example is offline and deterministic while its control flow is the one a
  live caller writes.
- The `examples-smoke` job now **runs** the two offline Rust examples instead of
  only compiling them. They are workspace members, so `cargo test --workspace`
  built them and nothing executed them — the assertions inside had never run.
  `ticker` stays excluded: it opens a socket to a live venue.

- **The sleep-and-retry loop that `Backoff` and `WeightedRateLimiter` were
  written for.** Both were complete, tested policies that nothing called:
  each had exactly one reference outside its own file, the `mod` line.
  `retry.rs` even said "the actual sleep-and-retry loop lives in the real
  transport adapter" — and that adapter did not have one. `ThrottledTransport`
  is that loop, written as a decorator over any `HttpTransport`, so it applies
  to all ten venues without one of them changing. `factory::transports` wraps
  the real socket transport in it, which is what makes it reached rather than
  merely available.
  - **A write is never repeated after a timeout.** `Error::is_retryable`
    includes `Timeout` and `Network`, and repeating a `POST /order` on either is
    how one order becomes two — the venue may have executed it. Only an explicit
    refusal (HTTP 429 / 418) is repeated on any method, because a refusal means
    nothing happened. Timeouts and dropped connections are retried on `GET`
    alone. A test pins each half of that rule.
  - A venue that states a wait in `Retry-After` gets that wait, not the policy
    curve — its number is the specific one. That is now readable at all, thanks
    to the response headers added alongside.
  - No request budget is configured by default. A capacity invented for a venue
    that publishes a different one either throttles traffic the venue would have
    accepted, or fails to protect against the limit it was meant to. The budget
    is there, weighed per request and tested; a caller who knows their account's
    limits opts in with `with_budget` and `with_weigher`.
  - The clock, the sleep and the jitter source are injected, so the loop is
    driven in tests with no real time passing.
- `Backoff::new` is `const`, so a caller can state its policy as a constant next
  to the reasoning for it.

- **Golden-fixture parity in every binding.** The committed replay tapes in
  `golden/` were driven by the Rust suite alone. Each binding had a replay test
  that proved a tape reaches a fill, and none of them checked the numbers — a
  lost decimal, a dropped fee or slippage applied to the wrong side would still
  produce a fill and still pass. Python, Node, Go, C#, Java, R and the C ABI now
  each drive the same two fixtures through the same fixed SMA strategy and
  assert the fill price and both balances the Rust suite pins
  (`102.0 / 1 BTC / 99898.0 USDT` frictionless, `102.102 / 1 / 99897.846949`
  with fees and slippage).
  - The strategy is reimplemented in each language rather than imported, so what
    is under test is the replay-to-paper-fill pipeline and not two libraries
    agreeing on a three-value mean.
  - C, Java and R read the fixtures with a small field reader written for that
    one committed shape. C ships no dependency at all, Java's only test
    dependency is JUnit, and the R package declares none; taking a JSON parser
    on so a test can read four numbers and one array would be a poor trade.

### Fixed

- **`Health` and `redact` were public, tested and undocumented.** Both are
  exported from the crate root, and neither appeared anywhere outside the source
  — `Health` had zero references in the entire repository apart from its own
  `pub use` line. They are caller-facing by the same test that separated
  `reconcile_orders` from the clock and the retry loop: the library cannot fill a
  `Health` for you, because the pull model already hands you every input
  (`Disconnected` / `Reconnected` for the connection and the reconnect count, the
  print timestamp for staleness, `sync_time`'s return for the clock offset, and
  the rate budget from the `ThrottledTransport` you wrapped the client with).
  `docs/STREAMING.md` gains the fold and says why `connected` alone is not
  enough — a stream that stopped delivering is still connected — and
  `docs/AUTH.md` covers redaction where the credentials it protects are already
  discussed. `examples/rust/src/health_and_redaction.rs` runs both offline
  against a replay tape, and the `examples-smoke` job runs it.
  Noted while writing it: only `TradePrint` carries a venue timestamp; `Ticker`,
  `BookSnapshot` and `BookDelta` are identified by update id, so a staleness
  clock over those events is the caller's own.

- **The README sold a feed that nothing subscribes to.** Under the
  differentiators it said funding, open interest, liquidations and long/short
  ratio "arrive as the exact typed shapes `wickra-core` consumes". They do not
  arrive at all: across all ten venue clients, `fundingRate`, `openInterest` and
  `premiumIndex` appear zero times. `FundingRate`, `OpenInterest`,
  `Liquidation`, `LongShortRatio`, `MarkIndex` and `DerivativesFeed` are six
  public types with no producer — the shapes and the `DerivativesTickBuilder`
  fold are real and tested, but the frames have to come from the caller.
  What *is* wired is the other half of the same sentence, and it stays: every
  client emits `TradePrint` and `OrderBookSnapshot` on its stream, and
  `trade_from_print` / `order_book_from_snapshot` / `cross_section` convert them
  into the core's input types with no glue. `README.md`, `ARCHITECTURE.md` and
  the `feeds` module doc now separate the two halves, and
  `docs/DERIVATIVES.md` gains a section saying which channels are subscribed and
  which are shapes waiting for data. Subscribing to the derivatives channels on
  eight futures venues is a feature, not a correction; naming the gap is what
  this entry does.

- **OKX signed with this machine's clock, and Upbit's nonce could repeat.**
  Two gaps in the clock work above, found by counting which clients actually
  hold the types involved rather than by re-reading the claim.
  - **OKX** had no `ServerClock` and no `sync_time` at all: eight of the ten
    clients had one. It stamps `OK-ACCESS-TIMESTAMP` on every signed request and
    refuses one whose time is too far from its own, so this is the exact failure
    the clock work set out to end — a machine a few seconds off has every signed
    call rejected, with a message about the timestamp rather than about the
    clock. It now reads `GET /api/v5/public/time` (the timestamp arrives as a
    string inside the envelope's one-element `data` array) and signs with the
    corrected value.
  - **Upbit's** JWT nonce was `wkex-{wall clock in ms}`. Upbit refuses a nonce it
    has seen, so two signed calls inside one millisecond meant the second was
    rejected. This is the same defect `NonceGenerator` was written for and fixed
    on Kraken — Kraken was the only venue using it. Upbit now uses it too.
  - Upbit still has no `ServerClock`, and deliberately: its JWT carries a nonce
    rather than a validity timestamp, so its signature does not depend on
    agreeing with the venue's clock. Nine of ten is the correct number here, and
    the entry above now says so instead of "each client".

- **Three documents described things that are not in the tree.**
  - `README.md` and `ARCHITECTURE.md` both listed `crates/wickra-exchange-cli/`
    — "the `wkex` command-line client" — in their project layouts. There is no
    such crate, no such workspace member, and no mention of a CLI in
    `ROADMAP.md`. A layout diagram is a description of the tree; both now
    describe it. (Every other path in both diagrams was checked and exists.)
  - `ARCHITECTURE.md` described the `observability` module as
    "tracing + redaction + health". There is no tracing anywhere in the
    workspace: `tracing` is declared in `[workspace.dependencies]` and no crate
    depends on it, and the repository contains zero log statements. The module
    is secret redaction and a health snapshot, and the line now says so, with
    the absence named rather than papered over.
- **`docs/CAPABILITIES.md` claimed a test that does not exist.** It said "a
  per-binding completeness test pins the canonical verb set so a dropped method
  fails CI". There is no such test in five of the seven bindings — only Python
  and Node assert the surface at run time. What actually guards it is
  `scripts/check_binding_surface.py`, which reads the verb set out of
  `traits.rs`, holds every binding's source to it, and runs in CI as its own
  job. The paragraph now says that, and names the WASM binding as deliberately
  outside the claim: it has no sockets and therefore no live client, so "full
  execution surface in every binding" was never meant to include it.

- **The R package could only be installed inside this repository's CI.** Its
  `Makevars` took the C ABI header and library from `WKEX_INC` / `WKEX_LIB`,
  environment variables that only this workflow sets, and baked no rpath — so
  the native library also had to be on the loader path at run time. Anyone
  running `install.packages()` got a compiler error about a missing header. It
  now ships `configure` / `configure.win`, which fetch the
  `wickra-exchange-c-<triple>.tar.gz` release asset for the package's version
  and stage it into `src/`, plus `install.libs.R`, which bundles the library
  beside the compiled object where the baked rpath (`$ORIGIN` /
  `@loader_path`, or the same directory on Windows) resolves it.
  `WKEX_INC` / `WKEX_LIB` still work as the local-build override.
  - The CI job no longer exports `LD_LIBRARY_PATH` / `DYLD_LIBRARY_PATH` /
    `PATH` before running the R suite. With them set, the job passed whether or
    not the package was self-contained; without them it passes only if it is,
    which is the property a user actually depends on.
  - `configure` refuses the Emscripten target with a message rather than
    failing further in. Unlike the indicator library, this one is a network
    client: the C ABI is built on tokio, reqwest and tokio-tungstenite, and
    `wasm32-unknown-emscripten` has no sockets for them. The offline simulators
    are published separately as the `wickra-exchange-wasm` npm package.
  - This is the prerequisite for the r-universe registry entry, which is the
    last open item in the repository blueprint. It stays deferred until the
    first release exists, because `configure` downloads a release asset and
    registering before then would only produce a red build.

- **The CodSpeed gate was comparing against the wrong commit, and the path
  filter on `main` was why.** CodSpeed measures a pull request against a
  measurement of `main`; when the newest `main` commit has none it falls back to
  an older one and says so in a footnote. The workflow only ran on pushes that
  touched `crates/**`, `Cargo.toml`, `Cargo.lock` or its own file, so every
  merge that changed bindings, examples or documentation left `main` unmeasured
  — three in a row at one point. Pull requests were then compared against a base
  several merges old, and everything in between was reported as *their*
  regression: `apply_delta` at 453.6 ns against a 399.4 ns base, on branches
  whose generated assembly for that function was byte-identical.
  - It was not the machine. The diagnostic step added alongside recorded the
    same CPU (Intel Xeon 6973P-C, family 6 model 173 stepping 1), the same
    kernel and the same rustc on both sides of one such comparison, which is
    what ruled the environment out and left the base selection.
  - The `push` trigger no longer filters by path, so every commit that lands on
    `main` is measured and the base is always the real one. The `pull_request`
    filter stays: a pull request that touches nothing relevant needs no run. The
    cost is one job per merge, against a gate that cried wolf until people
    stopped reading it.

- **`Error::RateLimited` never carried the wait it documents.** The variant has
  a `retry_after` field described as "the advised wait if the venue supplied one
  (`Retry-After`)". It was constructed thirteen times across the ten venue
  clients, every one of them with `None`; the only `Some` anywhere in the
  repository was in a test of the `Display` implementation. A caller reading the
  field to decide how long to back off always read nothing.
  - The cause was one layer down: `HttpResponse` carried only a status and a
    body, so the transport discarded the headers — and `Retry-After` is a
    header — before any venue client could see it. `HttpResponse` now carries
    them, with a case-insensitive `header` lookup (venues differ, and HTTP/2
    lower-cases names) and a `retry_after` accessor.
  - A rate limit is recognised from the *body*, a code in the venue's error
    envelope, while the wait arrives in the *header*. `Error::with_retry_after`
    joins the two at the call site that holds both, and refuses to overwrite a
    wait the venue already stated in its body, which is the more specific of the
    two.
  - Only the delta-seconds form of `Retry-After` is read. The HTTP-date form is
    legal, but no venue in this taxonomy sends it and parsing dates would put a
    date library and a clock inside a pure function; an unread `None` beats a
    wrong duration a caller would sleep for.
  - All ten venues have a test that pushes a rate-limited response carrying
    `Retry-After` and asserts the error arrives with the wait attached.

- **The C examples asserted nothing on Windows.** They double as the C-side test
  suite, and CI builds them with `--config Release` — which on a multi-config
  generator defines `NDEBUG` and compiles every `assert` in them away. The
  Windows runs were therefore green regardless of what the library did, while
  the Linux and macOS runs (single-config, no `NDEBUG`) really checked. All
  three now `#undef NDEBUG` before including the assert header.

- **A WASM binding, `bindings/wasm`, carrying the offline simulators.** The
  README previously said there would be none, on the argument that
  authenticated trading needs raw sockets and secret keys a browser forbids.
  That argument holds and is unchanged — what it did not cover is the part of
  this library that needs neither: `PaperExchange` and `ReplayExchange` are pure
  computation, and they live in `wickra-exchange-core`, which has no tokio,
  reqwest or tokio-tungstenite dependency. So the binding exists and is
  deliberately smaller than its siblings: paper and replay accounts, order
  placement, cancellation, lookup, balances, ticker and event draining. There is
  no `connect`, no user-data or WebSocket execution, no derivatives and no
  `klines`, because each needs a network.
  - There is also no `orderBook`: the paper account has no depth feed and
    answers `unsupported`, and replay delegates straight to it, so on both
    backends reachable from WASM the call cannot succeed. A method that
    type-checks and always throws is worse than an absent one.
  - `golden.test.js` drives the two committed replay tapes in `golden/` through
    the binding and asserts the same fill price and balances the Rust suite
    pins, so the JavaScript path cannot drift from the Rust one silently.
  - `p256` (Coinbase's ES256 request JWT) pulls in `getrandom`, which refuses to
    pick a backend on `wasm32-unknown-unknown`; the crate enables its `wasm_js`
    feature, selecting the Web Crypto backend.
  - `scripts/check_binding_surface.py` deliberately does not list this binding:
    it holds a binding to the *full* contract, which would report the smaller
    surface as a defect on every run. The reason is recorded next to the list.

- **`bindings/c/include/wickra_exchange.hpp`** — an optional, header-only C++
  layer over the C ABI. The ABI hands out five kinds of opaque handle, each
  released exactly once by its own `wickra_*_free`; every early return between
  the constructor and that call leaks one, and a thrown exception leaks it
  unconditionally. `wickra::Handle` is a move-only RAII owner that frees at
  scope exit however the scope is left, with an alias per handle type
  (`wickra::Exchange`, `Derivatives`, `Advanced`, `UserData`, `WsExecution`) so
  a handle cannot be paired with the wrong free function. Copying is deleted
  rather than defaulted: two owners of one handle would free it twice.
  `examples/c/paper.cpp` now uses it instead of a hand-written free at the end
  of `main`, so the header is compiled and run on all three CI operating
  systems rather than only existing. Unlike `wickra_exchange.h`, this file is
  hand-written and no generator touches it.

- **CodSpeed on every pull request.** `bench.yml` runs nightly and prints numbers
  a person has to read, so a slowdown is found by somebody re-measuring by hand,
  weeks after it landed. This counts instructions under instrumentation instead
  of timing wall clock, which is what makes a shared runner usable — the figure
  does not move because a neighbour VM got busy. The bench crate's `criterion`
  now resolves to `codspeed-criterion-compat` under that name; without the alias
  the job succeeds and measures nothing, and a green job that measured nothing
  looks exactly like a green job that found no regression.
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

- **The universal npm package was packing every platform's native binary.**
  `bindings/node/package.json` listed `npm` and `*.node` in `files`, and
  `napi artifacts` places the six freshly built `.node` binaries into
  `npm/<platform>/` immediately before the package is packed — so the one
  package that exists to carry *no* binary would have shipped all six, plus the
  six stub manifests that are published as packages in their own right. A local
  `npm pack --dry-run` already showed 5 MB of unpacked content and a
  `win32-x64-msvc` binary in the universal tarball. `files` now names only the
  loader, the type definitions and the two licence texts, which is what the
  platform-package layout assumes and what `wickra` ships.
- `bindings/node/src/lib.rs` is excluded from CodeQL after all. It was left in
  when the config was written, on the argument that it is hand-written here
  rather than generated as in `wickra`. The first runs settled it: five
  `rust/access-invalid-pointer` alerts, one per exported `#[napi]` class, each
  anchored on a `pub struct` line — and the file contains zero `unsafe`. The
  dereference is inside napi-derive's expansion, which CodeQL attributes back to
  the macro's source span, so the rule cannot say anything true about this file
  and the count grows with every class. Excluded by path rather than by
  disabling the query, which would also blind `bindings/c/src`, where the real
  raw-pointer code lives.
- **Client order ids could repeat, and a repeated one is a refused order.**
  `ClientIdGenerator` existed and was called by nothing. Where the caller
  supplied no `client_order_id`, three sites derived one from the wall clock in
  milliseconds — so two orders placed inside the same millisecond carried the
  same id, and Coinbase and KuCoin deduplicate on it. Two more derived it from
  the order's index within its batch, so the first order of *every* Bitget batch
  was `wbatch-0`. All five now draw from a generator seeded from the clock at
  construction and monotonic from there: the seed keeps two clients in one
  process apart, the counter keeps two orders in one millisecond apart.
- **Signed requests carry the venue's time, not this machine's.** `ServerClock`
  existed, was tested, and was called by nothing: all ten venue clients stamped
  signatures from the local clock. A venue refuses a signed request whose
  timestamp falls outside its receive window, so a machine a few seconds off had
  every order rejected — with a message about the window rather than about the
  clock. Nine of the ten clients gain `sync_time()`, which reads the venue's
  own time endpoint, and every signed path uses the corrected value. Upbit is
  the exception and needs none: its JWT carries a nonce rather than a
  validity timestamp, so nothing in its signature depends on agreeing with
  the venue's clock. The endpoint shapes
  were read off the live public endpoints rather than taken from documentation:
  Kraken reports seconds where everyone else reports milliseconds, and Bitget,
  OKX and Coinbase return the number as a string.
- **Kraken's nonce could repeat.** It was the wall clock in milliseconds, so two
  signed calls inside one millisecond produced the same nonce and a clock that
  stepped backwards produced a smaller one — both `Invalid nonce` from the
  venue, and neither visible here. `NonceGenerator` now forces it strictly
  upward whatever the clock does.
- **Kraken's WebSocket token was never renewed.** `GetWebSocketsToken` returns
  the token with `expires` (900 seconds) and that field was read past, so a
  client placing orders over the WebSocket API for longer than fifteen minutes
  kept sending one the venue had stopped accepting — `ensure_ws_api` would not
  refetch, because the *connection* was still open. `TokenTtl` now tracks it and
  the pair is replaced on expiry.
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
