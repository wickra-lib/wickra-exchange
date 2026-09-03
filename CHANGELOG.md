# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **OKX subscribes to the derivatives channels it publishes.** It is the third
  of the eight futures venues to implement `DerivativesStream`, after Binance
  and Bybit: `funding-rate` and `liquidation-orders` are subscribed, and open
  interest and long/short positioning are read from
  `/api/v5/public/open-interest` and
  `/api/v5/rubik/stat/contracts/long-short-account-ratio`.

  Two of OKX's differences are visible in what the client does, rather than
  hidden behind a uniform-looking API:

  **A combined mark/index subscription is refused.** OKX publishes the mark
  price on `mark-price` and the index price on `index-tickers`, under different
  instrument ids, and never in one frame. Joining them would report two prices
  observed at different moments as one simultaneous reading. Binance and Bybit
  carry both in a single frame and are unaffected.

  **Forced orders are subscribed per product, not per market.** One stream
  carries every liquidation on the product, so the client drops frames for
  markets that were not asked for rather than handing a caller watching BTC the
  venue's entire forced flow.

  The long/short reply is a *ratio* where Binance's is a pair of proportions;
  with two categories the conversion is exact, so the feed type carries
  proportions from every venue. Every field name and response shape here was
  checked against the live API rather than taken from documentation.

  The README no longer says no venue client subscribes to these channels, which
  stopped being true when Binance and Bybit landed.

### Changed

- **The shared reconnect says which of its four outcomes happened.** Every
  venue's `poll_events` reconnects through one function, and it was silent: a
  caller saw `Event::Disconnected` and then, sometimes, `Event::Reconnected`,
  with no way to tell the absence apart. No WebSocket transport configured, the
  reconnect failing to open, and the subscriptions failing to replay all look
  identical from outside and call for three different fixes — the first is a
  construction mistake, the second is the network, the third leaves a live
  socket subscribed to nothing.

  Each now logs, at `info` for the two normal outcomes and `warn` for the two
  that leave the stream closed. The URL is not logged: a private stream carries
  its credential in the path — Binance's user-data socket is
  `wss://.../ws/<listenKey>`, and a listen key opens the account's order and
  balance stream.

  The three failure outcomes now have tests, which none of them had: the mock
  WebSocket transport could only ever open a connection that accepted
  everything, so a refused reconnect and a reconnect whose subscriptions cannot
  be replayed were unreachable from a test. `MockWsTransport` gains
  `push_refused_connection` and `push_unsendable_connection` for them.

  `retry.rs` and `deadman.rs` were candidates and are deliberately left alone.
  Both are pure policy types with no I/O — `DeadMansSwitch` has no call site in
  the library at all, the caller holds it — so there is no decision there to
  report. `net.rs` is left alone for the opposite reason: its failures already
  reach the caller as `Error::Network`, so logging them would repeat what the
  return value says rather than recover something lost.

### Fixed

- **A Bitget futures client streamed spot.** Bitget v2 serves one WebSocket URL
  for every product and tells them apart by `instType`, the way its REST paths
  tell them apart by `productType`. The REST paths did; both subscribe paths
  hardcoded `SPOT`. So a futures client read futures over REST and watched the
  **spot** book and the **spot** trades over the socket — and its private
  user-data stream watched the spot account, where a futures order never
  appears at all.

  Nothing failed while it was wrong, which is why it lasted: the venue answers a
  spot subscription perfectly well, with the wrong market's data. Every
  subscription now names the client's own market, and a test pins both
  directions.

- **A reduce-only close sent over Binance's WebSocket opened a position.** The
  REST body and the WebSocket frame each resolved the position fields for
  themselves, and the frame only ever spelled the hedged `positionSide`: on a
  one-way futures account it never sent `reduceOnly` at all. An order the caller
  had marked "close, do not open" went out as an opening order — the opposite
  trade, on the venue where the WebSocket order API is most used. Both paths now
  resolve the same function, so they cannot drift again.

- **`reduce_only` is refused on every spot client, not silently dropped.** It is
  the one order field whose meaning comes from the account rather than the
  venue: a spot account holds balances, not positions, so there is nothing for
  it to close. Binance and Upbit already refused. Bitget, Coinbase, Gate, HTX,
  Kraken and KuCoin dropped it in silence, and Bybit and OKX sent it to a spot
  endpoint that does not apply it — which reaches the caller as the same
  falsehood from the other side. All ten now refuse, on the single-order, batch
  and WebSocket paths alike.

### Added

- **The README's test count is derived rather than maintained.** It had
  advertised 534 unit tests against 540, and before that 441 against 513: a
  hand-kept number drifts the moment someone adds a test without thinking of it,
  and nothing fails when it does — the suite passing says nothing about a
  sentence in another file. Both drifts were found by someone counting.
  `scripts/check_test_count.py` counts the test attributes in the crate, which is
  exactly the set `cargo test --lib` runs, and holds the README to it in CI. The
  sixth check script, and the third time this number had gone stale.

- **Every venue's parser is now checked against that venue's own recorded
  answer.** `testdata/` holds 27 replies — a ticker, some candles and a book
  from each of nine venues — exactly as they came off the wire, and
  `tests/recorded.rs` replays them offline in every CI run.

  The offline suite proves a parser reads what its author believed the venue
  sends; it cannot prove the belief was right, because the fixture and the
  parser were written by the same hand from the same reading. The live suite
  asks the real venue but needs a network and skips out loud when the runner is
  blocked, so it cannot be the only proof either. These recordings are the
  reproducible half.

  The recorder names no URL. It builds each venue's real client over a transport
  that wraps the real one and writes down whatever came back, so the endpoint
  recorded is the endpoint the client asks for — writing the URLs down would
  record what the author believes the client does, which is the failure the
  fixtures exist to rule out. A non-2xx reply is never recorded, so a geo-block
  cannot overwrite a good fixture.

  Coinbase is absent: its market endpoints are signed, so there is no public
  recording to take. The nine that are present are asserted as a list, so a
  fixture that vanishes fails rather than shrinking the check in silence.

- **The order-field contract covers all six fields and all three paths.** It
  held four fields on two paths: `time_in_force`, `post_only` and `stp` on the
  single-order and batch paths, with the WebSocket path under contract for the
  trigger price alone. `client_order_id` and `reduce_only` were outside it
  entirely, and the WebSocket frame — a third hand-written builder of the same
  order, in a protocol that shares no field names with the REST body — was
  checked for one field out of six.

  Two contracts close it: `the_websocket_path_carries_every_field_too` holds all
  five order sockets to the same table as the other two paths, and
  `reduce_only_is_carried_on_a_derivatives_client_and_refused_on_a_spot_one`
  states the field's account-dependent answer on all three paths. The Binance
  defect above is what the first one found on its first run.

  `client_order_id` is matched by its value rather than by its key: ten venues
  spell that key ten ways, and a list of ten spellings is a list to keep in step
  with ten clients, while the id itself is on the wire whichever key carries it.

- **A batched or socket-sent order carries every field in C#, Java and R.** The
  C ABI has offered `wickra_advanced_place_batch_full` and
  `wickra_ws_place_order_full` since `WickraOrderRequest` arrived, and only Go
  called them. The other three reached the narrow four-argument forms on both
  paths, so a batched or WebSocket order from those languages could be a market
  or a limit and nothing else — no trigger price, no time-in-force, none of the
  flags that decide what the order is — while their single-order path carried
  all six fields.

  C# gains a `PlaceBatch(IReadOnlyList<OrderRequest>)` overload and
  `PlaceOrderWsFull`; Java gains `placeBatchFull` and `placeOrderWsFull`; R
  gains `wkex_place_batch_full` and `wkex_ws_place_order_full`. The narrow forms
  remain as the shortest spelling of the common case. Each language now builds
  the request struct in one place rather than once per path, which is what let
  the paths differ to begin with.

- **`check_binding_surface.py` checks per order path, not just per field.** It
  asked whether a binding could name a field *anywhere*, and answered yes for
  all seven while three of them could not put that field in a batch — the same
  shape as the check that once counted verbs without asking what a verb could
  express. The fourth axis reports `3/3 order paths`, and a binding that reaches
  a field on one path but not another now fails rather than averaging out.
  `check_r_abi_skew.py` goes from 43 of 45 C ABI symbols reached to 45 of 45.

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

### Fixed

- **The mirrored Go module shipped a test that could not pass.** `go-mirror`
  copies `bindings/go/*.go` into the published `wickra-exchange-go`, which
  includes `golden_test.go` — and that file read its replay tapes from
  `../../golden`, two directories above the package. That path exists in this
  repository and nowhere in the module a consumer gets, so `go test ./...`
  after `go get` failed on a fresh checkout of a released module. The tapes now
  travel with the module and the test resolves either layout.

  The reason this stayed invisible is the more useful half: the release job's
  verification step ran `go test -run TestAssembledModuleSmoke`, one test **by
  name**, so the mirrored tests were compiled and never run. It now runs
  `go test ./...` against the assembled tree, which makes a module whose tests
  cannot run fail the release rather than the consumer's first command.

### Changed

- **`wickra-core` moved from the `0.9` line to `1.0`.** The pin admitted only
  0.9.x while crates.io serves 1.0.4, so the lockfile sat a major version
  behind and no `cargo update` could have reached across the boundary. The 1.0
  API needed nothing from this repository: the workspace compiles as it stood,
  and the suite and clippy pass untouched. Repositories that consume this one
  by git rev can now raise their own `wickra-core` pin without resolving two
  incompatible cores into a single graph.

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

- **`Ticker`, `OrderBookSnapshot` and `BookDelta` carry the venue's timestamp.**
  Only `TradePrint` did, so a consumer could see how old a trade was and not how
  old the quote or the book was — the difference between acting on the market
  and acting on a memory of it. `Health.last_message_ms` could likewise only be
  fed from trades.

  Every expression was verified against the venue's real response, fetched from
  the same endpoint the client calls, rather than read out of documentation:

  | | ticker | book |
  |---|---|---|
  | Binance | `closeTime` | futures `T`; spot publishes none |
  | OKX, Bitget | `ts` | `ts` |
  | HTX | envelope `ts` | `tick.ts` |
  | KuCoin | `data.time` | `data.time` |
  | Upbit | `trade_timestamp` | `timestamp` |
  | Gate | — | `update` |
  | Bybit | — | `ts` |
  | Kraken, Coinbase | — | — |

  A venue that publishes none reports `0`, never the local clock: a
  locally-stamped quote looks fresh by construction, which is the one thing a
  staleness check must never be told. Kraken stamps individual depth levels but
  never the book as a whole, and Coinbase's market endpoints need a key.

  The field is appended at the end of `WickraTicker`, so every C-ABI offset
  before it is unchanged, and it reaches all nine languages. The gated live
  suite now asserts *presence* per venue as well, so a venue that stops sending
  a stamp — or a parser that stops reading one — fails nightly rather than
  quietly reporting zero.

- **`tracing` is wired, having been declared and unused.** It sat in
  `workspace.dependencies` with no crate depending on it and not one log
  statement in the codebase. `ThrottledTransport` now traces its decisions,
  which is where they were least visible: a caller that waited two seconds could
  not tell whether the request budget held it, the venue refused it, or the
  network dropped it, and those three call for different fixes. Retries, venue
  cool-offs and budget waits log at `debug`; a repeat refused because the method
  is not safe to repeat logs at `warn`, since that is the case where a caller is
  left holding an order it cannot account for.

- **The venue clients are now checked against the venues.** The offline suite
  drives every client over a mock transport with hand-written JSON. It proves
  the parser reads what the author believed, and it cannot prove that belief
  matched the venue: the fixture and the parser were written by the same hand,
  from the same reading of the same documentation, so they agree whether or not
  the reading was right. That is not hypothetical — it is how seven clients came
  to never send `time_in_force` under a green suite, because the fixtures did
  not expect it either.

  `crates/wickra-exchange/tests/live_public.rs` asks the real venue instead. All
  ten clients, through `ticker`, `klines` and `order_book`, against the live
  public API with no credentials. It runs from the existing nightly
  `testnet.yml`, never on a push.

  A failure means one thing: the venue answered and the parser could not read
  the reply. Network errors, timeouts, rate limits, HTTP 451/403 geo-blocks and
  auth errors are skipped out loud, because they say nothing about the code —
  and because a nightly job that failed on a blocked runner IP would be switched
  off within a week, taking the drift detection with it.

  First run: nine of the ten venues verified end to end. Coinbase is skipped —
  its Advanced Trade market endpoints require an EC key, so there is nothing to
  read anonymously.

### Changed

- **Coverage measures the facade and the C ABI, not just the core.** The facade
  was excluded wholesale on the grounds that it holds the real-socket transport
  adapters. That was true of `net.rs` and not of `factory.rs` beside it: the
  `connect*` dispatch is ordinary offline logic with its own unit tests, and
  leaving it unmeasured left unmeasured the exact place a defect had already
  hidden — no binding could reach a futures market, because the factory built
  every client with `MarketType::Spot` hardcoded.

  The exclusion is now the file that is genuinely untestable offline (`net.rs`)
  rather than the crate around it. The C ABI joins too, since its tests are
  cargo tests. Python, Node and WASM stay out for a different reason and not
  because they do not matter: their tests are pytest and node:test, so measuring
  them here would compile their Rust and never run it — reporting a zero that
  means "not measured" while looking like "not tested".

  The reported project percentage will move when this lands. That is more code
  being measured, not less being tested.

- **The derivatives feeds have producers.** `feeds.rs` carried `FundingRate`,
  `OpenInterest`, `Liquidation`, `LongShortRatio`, `MarkIndex`,
  `DerivativesFeed` and `DerivativesTickBuilder` — 471 lines of public API that
  nothing in the crate ever constructed. Typed shapes with no data path.

  `DerivativesStream` fills them. Three channels are pushed and subscribed to
  (`Funding`, `MarkIndex`, `Liquidations`, arriving as the new
  `Event::Derivatives`); open interest and long/short positioning are polled and
  read, because no venue streams them and modelling them as subscriptions would
  have made the surface look symmetric and the data arrive never.

  Implemented on **Binance** and **Bybit**. The two venues publish these
  channels quite differently — Binance on dedicated streams, Bybit folded into
  the same `tickers` stream that feeds the ordinary ticker, as deltas — which is
  what the design had to absorb. One frame can answer two subscriptions, so the
  client emits only the prints actually subscribed to: a client watching prices
  does not start receiving funding prints it never asked for. OKX, Bitget,
  Gate.io, HTX, KuCoin and Kraken are not wired yet, and the trait is simply
  absent on them rather than present and silent.

- **Every binding can build the order the core supports.** `place_order` was
  present in all seven bindings, and the order it could build was a market or
  limit with a quantity and a price. The trigger price that makes a stop-loss a
  stop-loss, the time-in-force that says an order must not rest, `post_only`,
  `reduce_only`, self-trade prevention and the client order id that makes a
  retry idempotent all existed in the core and had no spelling in any language:
  the eight PRs of venue fixes before this one could not reach a binding user.

  The C ABI gains `WickraOrderRequest` and three calls that take it
  (`wickra_exchange_place_order`, `wickra_ws_place_order_full`,
  `wickra_advanced_place_batch_full`). Python, Node and WASM — which hold
  `OrderRequest` directly and were narrow only because nobody had widened them —
  gain the core's builders. C#, Go, Java and R each carry the struct in the
  shape that language already uses. `place_market` / `place_limit` remain
  everywhere; nothing that compiled before stops compiling.

- **`OrderRequest::with_stop_price`.** Setting a stop price was a struct literal
  and nothing else — there was no builder for it in any language, Rust included,
  which is why no binding could offer one. It promotes the order type along with
  the price, because a trigger price on an order whose type says it is not a
  trigger order is a field every venue ignores.

- **The binding-surface check gained a third axis.** Verbs and configuration
  both passed while no binding could place a stop-loss, because a verb being
  present says nothing about what it can express.
  `scripts/check_binding_surface.py` now reads the field list out of
  `OrderRequest` and holds every binding to it; on the previous commit it
  reports 2 of 6 fields reachable from Python.

- **The binding surface check now covers configuration, not only verbs.** The
  defect above hid behind it for the life of the project: `place_order` was
  present in all seven bindings and could not place a futures order in any of
  them, because counting verbs cannot see which API a verb points at.
  `scripts/check_binding_surface.py` now also reads each binding's exchange
  constructor and fails if `market`, `margin_mode` or `position_mode` is missing
  from it — wherever that language declares it, including Go's `Options` struct
  and Java's widest overload.
  The search is confined to the constructor deliberately. The first draft
  searched the whole binding source and was **false assurance**: `MarketType::Spot`
  appears in every binding *because of* the bug, so a file-wide check for a
  market spelling would have passed on the broken code. It was verified the
  other way round instead — removing the parameter from each binding in turn and
  confirming the check fails by name for that binding.
  Worth recording for whoever edits it next: `strip_prose` drops every
  `#`-prefixed line, because R's roxygen comments start with `#'`. That takes
  every C `#define` with it, so the header's `WICKRA_MARKET_*` constants are
  invisible to this script and the axis has to be recognised from the parameter
  in the signature.

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

- **A coin-margined or margin client traded the wrong market, silently.**
  `MarketType` has four variants and most clients distinguish two. What the rest
  did was not reject the market -- it was to fall through to a path built for a
  different one.

  On Binance, `is_futures()` tests `UsdMFutures` alone, so a `CoinMFutures`
  client took the *spot* base URL and the spot order path: an order meant for a
  coin-margined contract went to spot, was accepted there, and traded the wrong
  instrument. On Bitget, KuCoin, Gate, HTX and Kraken the mirror image happened
  -- `is_derivatives()` is true for both futures variants, so inverse contracts
  were routed to the USDⓈ-margined endpoints (Gate's `/futures/usdt/`, HTX's
  `/linear-swap-api/`). And `MarketType::Margin` resolved to spot on all ten:
  no client signs a margin-account order anywhere.

  This is the shape of #192, where every binding built a spot client and no
  caller could reach futures at all: a market type accepted, ignored, and
  quietly replaced. The factory now refuses what a client does not route, which
  covers all nine languages in one place because every binding reaches the
  library through it. Bybit (`inverse`) and OKX (`SWAP`, where linear and
  inverse differ by instrument id rather than endpoint) do route coin-margined
  contracts and are not refused.

- **The binding-surface check could delete whole functions as if they were
  prose.** Its string-literal regex escaped `\\.`, and `.` does not match a
  newline — so a Rust line-continuation backslash (`"...text \\` at end of line)
  never terminated the literal, the closing quote of the *next* string paired
  with it, and every quote after that was off by one. The effect was silent and
  large: the check reported 2 of 25 verbs present in a binding that had all 25,
  on the strength of an error message wrapped across two lines.

- **Seven of the ten clients never sent `time_in_force`.** `OrderRequest` has
  carried GTC / IOC / FOK since the first release and `docs/CAPABILITIES.md`
  promised all three across every venue, but only Binance, Bitget and Bybit put
  the field on the wire. Coinbase, HTX, Kraken, KuCoin, OKX and Upbit dropped it
  entirely, and Gate sent only its post-only spelling. An `Ioc` — "fill what you
  can now, cancel the rest" — was therefore placed as the resting `Gtc` the
  caller had asked it never to be, leaving an open order where none was wanted.

  Every client now spells the field the way its venue does: Kraken's
  `timeinforce`, KuCoin's `timeInForce`, Upbit's `time_in_force`, Gate's
  `ioc`/`fok` beside the existing `poc`, OKX's `ordType`, HTX's `<side>-<kind>`
  string and swap `order_price_type`, and Coinbase's `order_configuration` key.

  Where a venue genuinely cannot say it, the order is now **refused** rather
  than weakened: Kraken has no fill-or-kill, Coinbase has no `limit_limit_ioc`,
  and a market order has no FOK spelling on most venues (Binance rejects
  `timeInForce` on `MARKET` outright with -1106).

- **The same silence on `stp` and `post_only`.** Coinbase, HTX, Kraken and Upbit
  dropped the self-trade-prevention policy; those four now refuse it, since none
  of their order APIs carries the field. Where a venue spells post-only as a
  *value* of its time-in-force slot — Bybit's `PostOnly`, Bitget's `post_only`,
  Gate's `poc`, and the order types OKX, HTX and Binance use — asking for both
  used to resolve silently in post-only's favour and discard the time-in-force.
  That pair is now refused.

- **Every batch builder was weaker than the single-order path beside it.** PR
  #189 found Bitget's batch dropping `reduce_only`; the same drift sat on six
  more venues. Gate's batch lost `time_in_force`, `post_only`, `stp` **and**
  `reduce_only` while its own `place_order` sent them; Bitget, Bybit, KuCoin,
  OKX, Kraken and HTX lost `post_only` and `stp`. An order no longer means
  something different because it travelled in a batch.

- **Binance sent `reduceOnly=true` on spot orders.** The flag had no
  `is_futures()` guard, and spot rejects the unknown parameter with -1104. Spot
  holds balances rather than positions, so the request is now refused instead.

- **The field-fidelity contract now covers the class, not one field.**
  `tests/conformance.rs` held all ten clients to "a trigger order is carried or
  refused, never flattened" — the lesson of PR #190, applied to `stop_price`
  alone. Four new contracts widen it to the whole request: each field is carried
  or refused on the single-order path, on the batch path, when two fields land in
  one venue slot, and when a market order asks for fill-or-kill. `docs/CAPABILITIES.md`
  now carries the resulting per-venue table instead of a blanket promise.

- **A price crossed every binding as the binary double, not the number typed.**
  `types.rs` opens by saying prices and quantities are `Decimal` and never
  `f64`, "because exchanges reject mis-rounded values, and float drift loses
  money". Four bindings then converted back with `Decimal::from_f64_retain`,
  which keeps the *binary* expansion of the double rather than the decimal the
  caller wrote:

  | asked for | sent to the venue |
  | --- | --- |
  | `20000.15` | `20000.150000000001455191522832` |
  | `1.005` | `1.0049999999999998934185896356` |
  | `0.1` | `0.1000000000000000055511151231` |

  A price filter rejects that, and a tick check rounds it away from what was
  meant. `Decimal::from_f64` reads the number the caller wrote. Sixteen
  conversion sites in the C ABI, one each in the Python, Node and WASM entry
  helpers, and one in the KuCoin client, where a position size arriving as a
  JSON number was expanded the same way.
  Each binding gained a test pinning the exact strings, verified against the
  previous behaviour: reverting the C ABI conversion fails with
  `left: "20000.150000000001455191522832", right: "20000.15"`. The non-finite
  guards are unchanged — `from_f64` returns `None` for NaN and infinity exactly
  as before, and NaN keeps its separate meaning of "market order" on the C ABI's
  price argument.
  The WASM test asserts only the conversions and says why: that binding reports
  errors as a `JsValue`, and constructing one outside a wasm runtime aborts the
  process inside wasm-bindgen rather than failing a test. The abort is also what
  made the first draft look green locally — an aborting test never prints
  `test result: FAILED`, so a run checked by grepping for that string reports
  success. Checked by exit status since.

- **No binding could reach a futures market.** Every binding built its exchange
  client with `MarketType::Spot` hardcoded, so no caller in Python, Node, C,
  C++, C#, Go, Java or R could place a futures order, read a futures book, or
  cancel a futures order. The `Exchange` surface was complete in all of them and
  half of it pointed at the wrong API.
  It went unnoticed because `check_binding_surface.py` counts **verbs**, not
  reachable **configurations**: `place_order` was present everywhere, and the
  derivatives, advanced-orders, user-data and ws-execution constructors each
  chose their own market, so nothing looked missing from the outside.
  Every exchange constructor now takes the market, and the two per-order modes
  with it — six venues between them carry `margin_mode` or the position side on
  the order itself, so neither can be set after the first order. The C ABI's
  five connect entry points gained the codes, and the Go, C#, Java and R
  wrappers pass them; Python and Node take them as optional arguments, Go as an
  `Options` struct through a new `ConnectWith`, C# as optional parameters, Java
  as an overload, and R as defaulted arguments — so no existing call site
  changes.
  **Only spot and USDⓈ-margined futures are offered.** `MarketType` also has
  `CoinMFutures` and `Margin`, and no client routes either consistently: Binance
  treats coin-margined as spot outright, five venues route it to their USDT
  futures path, and only Bybit maps it to a genuine inverse category; `Margin`
  is routed nowhere. Offering them would hand a caller a name that does not
  describe where the order goes — the defect this parameter exists to end — so
  an unrouted market is refused rather than silently downgraded to spot.

- **Every WebSocket order dropped a flag its own REST path sends.** Smaller than
  the trigger defect above and the same shape: the request carried a field, the
  REST body honoured it, and the WebSocket frame on the same client left it out.
  Nothing inverts the order — each one weakens it. A self-trade-prevention
  policy that is not sent is not applied, and a post-only order that loses the
  flag can take liquidity and pay the taker fee.
  Comparing each `place_order_ws` against the REST path it can actually reach:
  Binance, Bybit and Gate dropped `stp`; OKX dropped `stp` and `reduce_only`;
  Kraken dropped `post_only`.
  On four of the five the fix is a mirror, and the evidence for it is in the
  file: those frames already use the venue's REST field names throughout
  (`instId`/`tdMode`/`clOrdId` on OKX, `currency_pair`/`time_in_force` on Gate),
  so the REST mapping function applies unchanged.
  **Kraken is refused instead.** Its v2 frame names every field differently —
  `order_qty`, `limit_price`, `cl_ord_id` — so the REST spelling `oflags=post`
  proves nothing about the WebSocket one, and guessing it would be the same
  mistake in a smaller font. A post-only order over Kraken's WebSocket now
  returns `Error::Exchange` pointing at REST.
  (Gate's WebSocket path is spot-only and Kraken's is guarded to spot by
  `ensure_ws_api`, so both are compared against their spot REST body rather than
  the futures one — measuring against an unreachable path would have invented
  two more findings than exist.)

- **A stop-loss was placed as an order that executed immediately.** The most
  severe of the defects in this run, and the simplest: `OrderRequest::validate`
  *requires* a `stop_price` on a `StopMarket`/`StopLimit` — so the library
  accepted the order and asserted the trigger was there — and then every venue's
  `order_type_str` mapped `StopMarket` down to `"market"` and `StopLimit` to
  `"limit"` and sent no trigger at all. A stop-loss at 19 000 with the market at
  20 000 went out as a market sell and filled at 20 000: exactly the loss it
  existed to prevent.
  Counting the order-building paths in the crate: **one of thirty-one** sent a
  trigger price, `Binance::place_order`. The other thirty dropped it, including
  Binance's own batch and WebSocket paths. (The `stop_price` references in the
  OKX and KuCoin clients are `OcoRequest`, a different type on the bracket path,
  which was never affected.)
  Binance now carries `stopPrice` on all three of its order paths — the REST
  mapping mirrored into the batch entry and the ws-api frame. **Every other
  venue refuses a trigger order** with `Error::Exchange` code `unsupported`,
  rather than placing the immediate order underneath it. Implementing trigger
  orders natively on nine venues means nine different endpoints and parameter
  sets; refusing is correct today, and each venue can move to carrying them one
  at a time. `PaperExchange` already refused, and had done so from the start.
  `tests/conformance.rs` now holds all ten clients to the contract — a trigger
  order is either sent with its trigger or refused, never flattened — so a venue
  added without either is caught, and one that gains native support later moves
  between branches without the test changing.
  `docs/CAPABILITIES.md` claimed "all order types are common across venues:
  market, limit, stop-market, stop-limit". It now says which one is not.
  Codecov caught the first draft leaving the new guards untested on the batch
  and WebSocket paths — the two places the earlier `reduce_only` drop had hidden
  in as well. The conformance contract now drives all three order paths, and the
  per-venue suites gained the cases whose branches were never entered: Binance's
  trigger price on the batch entry and the ws-api frame, Gate's `close_short`
  half of dual mode, and post-only and client-id handling on Bybit, OKX and
  Bitget.

- **A hedged account got orders that named no side.** `position_mode` was the
  last field on `ExchangeOptions` that nothing read. On a hedged account a
  symbol holds a long and a short position at once, so every order has to name
  the one it acts on — under a different field name per venue, and on two of
  them *instead of* `reduce_only` rather than alongside it. None was sent, so a
  caller who configured `Hedge` had every futures order rejected (Binance
  `-4061`) or applied to the wrong side.
  - **Binance** now sends `positionSide` and drops `reduceOnly`, which the venue
    refuses in the same order (`-1106`), on all three futures order paths: REST,
    the native batch endpoint, and the ws-fapi WebSocket API.
  - **Bybit** sends `positionIdx` (1 = buy side, 2 = sell side) on the REST,
    WebSocket and batch paths; `0` is one-way and is the venue default, so it is
    left off rather than sent.
  - **OKX** sends `posSide` and drops `reduceOnly` on all four order paths,
    including the OCO algo order — a bracket protects a position that is
    already open, so it acts on the side its own side closes.
  - **Bitget** sends `tradeSide` (`open`/`close`) instead of `reduceOnly`.
  - **Gate.io** closes with `auto_size` (`close_long`/`close_short`) and `size`
    0, which is how its dual mode names a side; opening is unchanged, because
    the sign of `size` already says which side grows.
  - **HTX** needed no branch and gets none: its swap orders already carry
    `direction` **and** `offset`, which is the hedged encoding, and the venue
    takes the same shape in one-way mode.
  - **KuCoin Futures and Kraken Futures** hold one net position per contract and
    have no hedge mode. A futures order from a client configured `Hedge` now
    returns `Error::Exchange` instead of moving the net position — the same
    treatment `set_margin_mode(Isolated)` already gets on those two venues.
  The side is derived rather than asked for: buying opens the long side or
  closes the short one, and `reduce_only` separates the two. That mapping is
  `PositionSide::for_order`, written once and applied identically everywhere.

- **Bitget's batched orders lost their `reduce_only`.** Found while wiring the
  above: the batch entry builder set side, type, force, size, client id and
  price, and nothing else — so an order batched with `reduce_only` was sent as
  an opening one. It now carries the position half like the single-order path.

- **`ExchangeOptions.margin_mode` was read by nothing, and two venues carry the
  mode on every order.** Of the eight fields on `ExchangeOptions`, six are read
  — `market_type`, `testnet` and `recv_window_ms` by the clients, and `timeout`,
  `user_agent` and `proxy` by `ReqwestHttpTransport`. `margin_mode` was read by
  nothing at all, and on OKX and Bitget that is not cosmetic: the mode travels
  on the order, and the value on the order is the one that applies.
  - **OKX** derived `tdMode` from the market type alone, so every non-spot order
    went out as `cross` — on all four sites that build one (REST, WebSocket,
    batch, and the OCO algo order). It is now a function of the market *and* the
    configured margin mode: spot stays `cash`, and futures follow the option.
  - **Bitget** wrote `"marginMode": "crossed"` as a literal in both futures order
    paths, single and batch.
  - `set_margin_mode` on both venues changed the account setting and left the
    per-order value stale, so the very next order overrode what had just been
    set. Both now update the client's mode as well; both signatures became
    `&mut self` to do it, matching the `Derivatives` trait they implement.
  A caller who asked for isolated margin got cross: the whole account balance
  behind a position that was meant to be capped, with nothing said. It survived
  because `ExchangeOptions::mainnet` defaults to `Cross` and no test had ever
  constructed one with `Isolated` — seven new tests do, and each fails against
  the previous behaviour.
  `position_mode` remains the one field nothing reads; it is a separate fix.

- **The CodSpeed artefact is not the runner CPU, and the entry that said it was
  is corrected here.** The previous entry read the `apply_delta` figure as
  bimodal per CPU: base on an EPYC 7763 at 399.4 ns, head on an EPYC 9V74 at
  453.6 ns, everything else equal. The next occurrence had the CPUs the other
  way round — base on the 9V74 at 399.4, head on the 7763 at 455 — so one EPYC
  7763 run produced each of the two figures and the machine explains neither.
  Nor is it "head runs measure high": two pull requests in between reported no
  change at all, one of them against the same base commit.
  What *is* established, by the method the first incident introduced: the code
  does not move. Built with `--emit=asm` on both revisions, `apply_delta` and
  `apply_level` — the two functions the benchmark executes — are byte-identical
  between `main` and the pull request's head once basic-block label numbering is
  normalised. The figure moves while the instruction stream does not, and the
  cause is not known. The workflow comment now says that, and says how to check
  the next one: compare the recorded CPUs, look for the report's footnote naming
  a base commit older than `main`'s head, and diff the emitted assembly for the
  function named. No threshold was added, for the same reason as before — one
  wide enough to swallow 12% would swallow a real 12% regression.

- **A public type nobody could name, and three doc links to nothing.**
  `MockWsConnection` was exported from the crate root, but `connect` hands it
  back as a `Box<dyn WsConnection>`, its fields are private and it has no
  constructor — so no caller could name or build one. It is `pub(crate)` now,
  which is what `unreachable_pub` says it always was.
  Separately, three intra-doc links in public documentation resolved to private
  items: `PaperExchange` and `ReplayExchange` each pointed at `[module docs]
  (self)` in a private module, and Kraken's `subscribe_user_data` pointed at a
  private method. All three are prose now. They survived because **nothing ran
  rustdoc**: a broken intra-doc link is a warning, and the workspace's three
  linters were two. `cargo doc` with `RUSTDOCFLAGS: -D warnings` now runs in the
  lint job, over the two published crates rather than `--workspace` — the C
  binding's lib is also named `wickra_exchange` and the two collide on the
  output path.

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
  The redaction guidance changed while writing it, and CodeQL is why: the first
  draft assembled a request line holding the key and scrubbed it afterwards,
  which `rust/cleartext-logging` flagged as high severity on the pull request.
  It was right about the shape. The unredacted string exists first, and no
  analyser can prove the scrub. Both pages now teach the case `redact` actually
  exists for — a venue error body that quotes back the signature it rejected, a
  string the process never assembled — and say plainly: redact what arrives, do
  not interpolate what you hold.
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
