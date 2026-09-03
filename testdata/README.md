# Recorded venue answers

Each file here is one venue's own reply to one public read, as it came off the
wire. Nothing in this directory was written by hand.

```
testdata/<venue>/{ticker,klines,order_book}.json
```

## Why these exist

The offline suite drives every client over a mock transport with hand-written
JSON. It proves the parser reads what the author believed the venue sends. It
cannot prove the belief was right: the fixture and the parser were written by
the same hand, from the same reading of the same documentation, and they agree
whether or not that reading was correct.

That gap is not hypothetical. It is how seven clients came to never send
`time_in_force` while the suite stayed green — the fixtures did not expect the
field either.

`crates/wickra-exchange/tests/live_public.rs` closes the other half by asking
the real venue, and it is the right tool for noticing upstream drift. What it
cannot be is *reproducible*: it needs a network, and it skips out loud when the
runner is rate-limited or geo-blocked. A test that sometimes verifies nothing
cannot be the only proof that a parser reads real venue output.

So the real answers are recorded once and replayed offline, in
`crates/wickra-exchange-core/tests/recorded.rs`, which runs in every CI job.

## Refreshing them

```
cargo test -p wickra-exchange --test record_fixtures -- --ignored --nocapture
```

The recorder does **not** name a single URL. It builds each venue's real client
over a transport that wraps the real one, asks that client for a ticker, some
candles and a book, and writes down whatever came back. The URL is the one the
client uses, because the client chose it — writing the URLs down here would
record what the author believes the client asks for, which is the failure these
files exist to rule out.

A non-2xx answer is not recorded: a geo-block or a rate limit must not overwrite
a good recording.

## What a failure means

`every_recorded_venue_still_parses_its_own_answer` fails when a parser can no
longer read a reply the venue actually sent. Refreshing the fixture is the right
fix only once the venue is known to have changed. Until then the parser is
wrong about the venue, and the recording is the evidence.

## Coinbase

Absent on purpose. Its market endpoints are signed, so there is no public
recording to take without a key. `live_public.rs` notes the same exclusion for
the same reason.
