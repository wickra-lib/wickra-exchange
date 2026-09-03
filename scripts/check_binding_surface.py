#!/usr/bin/env python3
"""Assert that every binding exposes the same API surface, and can configure it.

Each binding is written separately and each has its own test suite, so a method
that goes missing in one of them fails nowhere: nothing compares the bindings
*to each other*. The C# example in this repository called `Balances()` for
months, a method the C# binding has never had, and CI was green throughout.

The Rust traits in `crates/wickra-exchange-core/src/traits.rs` are the source of
truth -- every binding is a consumer of them -- so this reads the trait methods
out of that file and checks each binding's public surface against them, spelled
the way that language spells it.

Two deliberate divergences are encoded rather than papered over, because they
are real and documented:

  * The C ABI cannot express an `OrderRequest`, so `place_order` arrives as
    `place_market` / `place_limit`. Every language that consumes the C ABI (C,
    C++, C#, Go, Java, R) inherits that split.
  * The C ABI returns one balance at a time (`balance`), where the native Rust,
    Python and Node surfaces return the whole map (`balances`).

Bindings may expose more than the contract -- a language idiom is not drift --
so this checks that the contract is present, not that nothing else is.

Run from the repository root:  python scripts/check_binding_surface.py
"""

from __future__ import annotations

import os
import re
import sys

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
TRAITS = "crates/wickra-exchange-core/src/traits.rs"


def read(path: str) -> str | None:
    full = os.path.join(ROOT, path)
    if not os.path.isfile(full):
        return None
    with open(full, encoding="utf-8") as handle:
        return handle.read()


def present(haystack: str, spellings: tuple[str, ...]) -> bool:
    """Is any spelling in the (already lowercased) source?

    Each spelling is tried in two forms: with underscores (C, R, Python) and
    without (PascalCase in C# and Go, camelCase in Java and Node, all flattened
    by the caller's lowercase). A word boundary cannot be used on the left --
    `_` is a word character, so a boundary-anchored `klines` never matches
    inside `wickra_exchange_klines`. On the right a boundary is essential:
    without it `connect` matches `connect_derivatives`, and the R binding passed
    a constructor check while having no `wkex_connect` at all.
    """
    for spelling in spellings:
        for form in (spelling.lower(), spelling.lower().replace("_", "")):
            if re.search(re.escape(form) + r"(?![a-z0-9_])", haystack):
                return True
    return False


def strip_prose(text: str) -> str:
    """Drop comments and string literals before matching.

    Prose mentions a method by name constantly, and a grep that counts those
    reports a surface the binding does not have. Both forms bit here: the R
    binding has no `wkex_connect`, yet a naive search found `connect` in a
    roxygen line and, after comments were stripped, in four error messages --
    `Rf_error("wickra: failed to connect derivatives client")`.

    Comments cover `/* */`, `//` (so Rust's `///`, Java's `/** */`, C#'s `///`)
    and `#` (so R's roxygen `#'`). String literals cover double-quoted spans,
    honouring backslash escapes.

    The escape class is `\\[\\s\\S]` rather than `\\.`, because `.` does not match a
    newline and Rust's line-continuation escape *is* a backslash followed by
    one. With `\\.` such a literal never terminated, so the closing quote of the
    *next* string paired with it instead -- and every quote after that was off
    by one, which silently deleted whole functions as if they were prose. The
    surface check then reported 2 of 25 verbs present in a binding that had all
    25, on the strength of a message wrapped across two lines.
    """
    text = re.sub(r"/\*.*?\*/", " ", text, flags=re.S)
    text = re.sub(r"(?m)//[^\n]*", " ", text)
    text = re.sub(r"(?m)#[^\n]*", " ", text)
    return re.sub(r'"(?:\\[\s\S]|[^"\\])*"', ' "" ', text)


def read_all(*paths: str) -> str:
    """Concatenate every file that exists, so a binding split across files is
    read as one surface. Comments and string literals are stripped: only
    code counts as surface."""
    out = []
    for path in paths:
        full = os.path.join(ROOT, path)
        if os.path.isdir(full):
            for base, _, names in os.walk(full):
                for name in sorted(names):
                    with open(os.path.join(base, name), encoding="utf-8",
                              errors="ignore") as handle:
                        out.append(handle.read())
        else:
            text = read(path)
            if text is not None:
                out.append(text)
    return strip_prose("\n".join(out))


def trait_verbs() -> dict[str, list[str]]:
    """The contract, read from the traits rather than from a list kept by hand."""
    text = read(TRAITS)
    if text is None:
        raise SystemExit(f"{TRAITS} not found; run this from the repository root")
    text = re.sub(r"(?m)^\s*//[^\n]*\n", "", text)

    traits: dict[str, list[str]] = {}
    for match in re.finditer(r"pub trait (\w+)[^{]*\{", text):
        name = match.group(1)
        index, depth = match.end(), 1
        while depth:
            depth += (text[index] == "{") - (text[index] == "}")
            index += 1
        body = text[match.end():index - 1]
        traits[name] = re.findall(r"(?m)^\s{4}fn (\w+)", body)
    if not traits:
        raise SystemExit(f"no traits found in {TRAITS}")
    return traits


def snake_to_camel(verb: str) -> str:
    head, *tail = verb.split("_")
    return head + "".join(part.title() for part in tail)


def snake_to_pascal(verb: str) -> str:
    return "".join(part.title() for part in verb.split("_"))


# How the C ABI spells each canonical verb. Three conventions are at work and
# all three are deliberate, so they are recorded here rather than reported as
# drift every run:
#
#   * `place_order` arrives as `place_market` / `place_limit`, because the C ABI
#     cannot express an `OrderRequest`, and `balances` narrows to `balance`,
#     because it returns one asset at a time.
#   * Symbols are grouped by *handle* rather than by verb --
#     `wickra_user_data_subscribe`, not `wickra_subscribe_user_data` -- so the
#     word order inverts for everything that hangs off a secondary handle.
#   * A language that models the handle as a type drops the handle from the
#     method name again: C# has `UserData.Keepalive()`.
#
# Every alias below was verified against the header and each binding; none is a
# guess that makes the check pass.
C_ABI_ALIASES = {
    "place_order": ("place_market", "place_limit"),
    "balances": ("balance",),
    "cancel_order": ("cancel",),
    "poll_events": ("poll",),
    "positions": ("positions", "position"),
    "subscribe_user_data": ("subscribe_user_data", "user_data_subscribe"),
    "keepalive_user_data": ("keepalive_user_data", "user_data_keepalive", "keepalive"),
    "place_order_ws": ("place_order_ws", "ws_place_order"),
    "cancel_order_ws": ("cancel_order_ws", "ws_cancel_order"),
}


def c_abi_spellings(verb: str) -> tuple[str, ...]:
    return C_ABI_ALIASES.get(verb, (verb,))


# (label, source paths, how a verb is spelled there)
#
# Python and Node bind the Rust surface directly, so they carry the canonical
# names. C#, Go, Java and R sit on the C ABI and inherit its spelling; each is
# matched case-insensitively against its own source, which covers PascalCase
# (C#, Go), camelCase (Java) and the wkex_ prefix (R) without a table per
# language that would go stale on its own.
#
# `bindings/wasm` is deliberately absent and must stay absent. It targets
# wasm32-unknown-unknown, where there are no sockets and therefore no live venue
# client, so it carries the offline subset (paper, replay) on purpose. This check
# holds a binding to the *full* contract; adding wasm here would report a
# deliberate scope as a defect, every time, forever.
BINDINGS = [
    ("python", ("bindings/python/src/lib.rs",), lambda v: (v,)),
    ("node", ("bindings/node/src/lib.rs",), lambda v: (snake_to_camel(v), v)),
    ("c", ("bindings/c/include/wickra_exchange.h",), c_abi_spellings),
    ("csharp", ("bindings/csharp/WickraExchange",), c_abi_spellings),
    ("go", ("bindings/go/wickra.go",), c_abi_spellings),
    ("java", ("bindings/java/src/main/java/org/wickra/exchange",), c_abi_spellings),
    ("r", ("bindings/r/R", "bindings/r/src"), c_abi_spellings),
]

# Spot-only venues do not implement the derivatives surface, and the paper and
# replay backends implement neither derivatives nor the private streams. That is
# a per-venue fact, not a per-binding one: every binding exposes the whole
# contract, and a venue that lacks a capability says so at runtime.
REQUIRED_TRAITS = [
    "MarketData", "Execution", "Exchange",
    "Derivatives", "AdvancedOrders", "WsUserData", "WsExecution",
]

# Verbs are only half the surface: a binding that exposes every method and no way
# to build a live client exposes nothing usable. The R binding shipped exactly
# that -- `wkex_paper` and `wkex_replay_trades` but no `wkex_connect` -- so an R
# user could reach the derivatives and user-data handles, which connect
# internally, and never construct a plain live exchange. The verb check passed
# throughout, because a constructor is not a trait method.
#
# Named as the C ABI names them; the same two-form matching applies.
CONSTRUCTORS = [
    "paper_new",
    "replay_new",
    "connect",
    "connect_derivatives",
    "connect_advanced",
    "connect_user_data",
    "connect_ws_execution",
]

# How each language spells a constructor. Python and Node expose them as static
# methods with their own names; the C-ABI consumers keep the C spelling.
CONSTRUCTOR_ALIASES = {
    "python": {"paper_new": ("paper",), "replay_new": ("replay_trades",)},
    "node": {"paper_new": ("paper",), "replay_new": ("replayTrades", "replay_trades")},
}

# The configuration axes the *exchange constructor* must expose, not just the
# verbs a binding must have.
#
# This exists because the verb check above passed for months while half the
# `Execution` surface was unreachable in every binding: each built its exchange
# client with `MarketType::Spot` hardcoded, so `place_order` was present
# everywhere and could not place a futures order anywhere. Counting verbs cannot
# see that -- the method is there, and it is pointed at the wrong API.
#
# An axis is listed when choosing it wrongly sends the request somewhere else,
# or sends a different order than the caller asked for:
#
#   * `market` decides which of a venue's APIs every later call is routed to.
#   * `margin_mode` travels on the order itself on OKX and Bitget, so a client
#     that cannot be told it trades cross whatever the account is set to.
#   * `position_mode` names the side of a hedged account an order acts on, and
#     four venues reject or misapply an order that omits it.
#
# The last two cannot be set after construction: the first order already carries
# them. That is what makes them constructor arguments rather than later calls,
# and what makes their absence a defect rather than a missing convenience.
#
# The search is deliberately confined to the constructor's own parameter list.
# A whole-file search does not work and is worse than nothing: `MarketType::Spot`
# appears in every binding's source *because* of the bug, so a file-wide check
# for a market spelling would have passed on the broken code. A check that
# passes today and would not have caught yesterday's defect manufactures
# assurance, which is the failure this whole file exists to prevent.
CONFIGURATION = ("market", "margin_mode", "position_mode")

# The third axis: what an *order* can say.
#
# Verbs and configuration were both checked and both passed while every binding
# could place only a market or limit order with a quantity and a price. The
# trigger price that makes a stop-loss a stop-loss, the time-in-force that says
# an order must not rest, post-only, reduce-only, self-trade prevention and the
# client order id that makes a retry idempotent all existed in the core and had
# no spelling in any language. `place_order` was present in all seven bindings
# the whole time -- the verb was there, the order was not.
#
# So this checks the same thing one level down: a binding exposes `place_order`,
# and every field an order carries can be reached through it. The fields are read
# from `OrderRequest` in the core rather than listed here, so a field added there
# is a field this demands.
ORDER_REQUEST = "crates/wickra-exchange-core/src/types.rs"

# The fields that need a way in. `symbol`, `side`, `order_type` and `quantity`
# are excluded: every binding already takes those positionally, in the
# constructor or the factory, and none of them was ever the gap.
ORDER_FIELD_SKIP = frozenset({"symbol", "side", "order_type", "quantity", "price"})

# How each field may be spelled. A binding that reaches it under any of these
# names counts; the list is per field rather than per language because the
# languages agree on the words and differ only in case, which `present` folds.
ORDER_FIELD_SPELLINGS = {
    "stop_price": ("stop_price", "stopprice", "R_STOP_PRICE"),
    "time_in_force": ("time_in_force", "timeinforce", "TIF_GTC"),
    "client_order_id": ("client_order_id", "clientorderid", "cliordid"),
    "reduce_only": ("reduce_only", "reduceonly"),
    "post_only": ("post_only", "postonly"),
    "stp": ("stp",),
}


# The three builders that send an order, and how each binding reaches the form
# of it that carries a whole `OrderRequest`.
#
# The field axis above asks whether a binding can *name* a field anywhere. That
# is not the same question as whether the field can reach the venue on the path
# the caller chose, and the difference was a real gap: C#, Java and R each
# scored a full six order fields while their batch and WebSocket calls took
# market, side, quantity and price and had nowhere to put the rest. The C ABI
# had carried `_full` forms of both since #196; only Go called them. A batched
# stop-loss was unplaceable from three languages, and this check said the
# contract was whole.
#
# So the axis is per path. Python and Node take the request type directly and
# are matched on that; the four C-ABI languages are matched on the `_full`
# symbols, which is the only way through for them.
ORDER_PATH_SPELLINGS = {
    "single": {
        "python": ("place_order",),
        "node": ("place_order",),
        "*": ("exchange_place_order", "place_order"),
    },
    "batch": {
        "python": ("place_batch",),
        "node": ("place_batch",),
        "*": ("advanced_place_batch_full", "place_batch_full"),
    },
    "websocket": {
        "python": ("place_order_ws",),
        "node": ("place_order_ws",),
        "*": ("ws_place_order_full", "place_order_ws_full"),
    },
}

# Python and Node hold `OrderRequest` itself, so their batch and socket calls
# take one by construction and there is no narrow twin to confuse them with.
# The check still has to prove it rather than assume it: these patterns say the
# request type appears in that method's own signature.
NATIVE_PATH_SIGNATURE = {
    ("python", "batch"): r"fn place_batch[^{]{0,300}?PyOrderRequest",
    ("python", "websocket"): r"fn place_order_ws[^{]{0,300}?PyOrderRequest",
    ("node", "batch"): r"fn place_batch[^{]{0,300}?OrderRequest",
    ("node", "websocket"): r"fn place_order_ws[^{]{0,300}?OrderRequest",
}


def order_paths(label: str, haystack: str, source: str) -> list[str]:
    """The order paths this binding cannot send a full request on."""
    absent = []
    for path, spellings in ORDER_PATH_SPELLINGS.items():
        signature = NATIVE_PATH_SIGNATURE.get((label, path))
        if signature is not None:
            if re.search(signature, source, re.S) is None:
                absent.append(f"{path} (no `OrderRequest` in its signature)")
            continue
        forms = spellings.get(label, spellings["*"])
        if not present(haystack, forms):
            absent.append(f"{path} (as {'/'.join(forms)})")
    return absent


def order_fields():
    """The fields of `OrderRequest`, read from the core rather than listed."""
    source = read(ORDER_REQUEST)
    body = re.search(r"pub struct OrderRequest \{(.*?)\n\}", source, re.S)
    if body is None:
        raise SystemExit(f"{ORDER_REQUEST}: could not locate `struct OrderRequest`")
    found = re.findall(r"^\s*pub (\w+):", body.group(1), re.M)
    fields = [f for f in found if f not in ORDER_FIELD_SKIP]
    missing = [f for f in fields if f not in ORDER_FIELD_SPELLINGS]
    if missing:
        raise SystemExit(
            "OrderRequest gained a field with no spelling recorded here: "
            + ", ".join(missing)
            + " -- add it to ORDER_FIELD_SPELLINGS and to every binding."
        )
    return fields

# Where each binding declares the axes, and the spellings they may take there.
# A binding whose declaration cannot be located fails loudly rather than passing
# quietly.
#
# Not every language puts them in a parameter list, and that is idiom rather
# than drift: Go carries them in an `Options` struct, which is how a Go API with
# more than a couple of knobs is written. So the pattern names the place, per
# language, instead of assuming one shape. Where a language offers overloads --
# Java keeps the old six-argument `connect` delegating to the new one -- every
# match is considered and the widest wins, since the narrow one is the
# convenience wrapper.
#
# The captures exclude their own delimiters -- `[^()]*`, `[^{}]*` -- rather than
# using a lazy `.*?`. A lazy capture is not bounded by the construct it starts
# in: given a `connect(` whose return type does not match, it runs on to the
# next one and swallows whatever lies between, which is how a removed parameter
# still appeared to be present.
CONSTRUCTOR_SIGNATURE = {
    "python": r"fn connect\(([^()]*)\)\s*->\s*PyResult<Self>",
    "node": r"pub fn connect\(([^()]*)\)\s*->\s*napi::Result<Self>",
    "c": r"WickraExchange \*wickra_connect\(([^()]*)\);",
    "csharp": r"static Exchange Connect\(([^()]*)\)",
    "go": r"type Options struct \{([^{}]*)\}",
    "java": r"static Exchange connect\(([^()]*)\)\s*\{",
    "r": r"wkex_connect <- function\(([^()]*)\)\s*\{",
}

# The three order numbers, and how each binding spells the exact form of them.
#
# The axes above ask whether a field can be named, a path reached, a market
# chosen. None asks whether the *number* that arrives is the number the caller
# wrote -- and every binding was sending order numbers through a double, which
# holds about fifteen significant digits where the core holds an exact decimal.
# `12345678.90123456789` arrived as `12345678.90123457`: a different order,
# placed without a word.
#
# What "exact" means differs by language and the spellings say so. Python has
# `decimal.Decimal` and an unbounded `int`; C# has `decimal`; Java has
# `BigDecimal`; C, Go and R have nothing, so the exact spelling is text. Node
# has one number type and it is a double, so it is text there too -- which is
# what every exchange's own API takes, for this reason.
EXACT_NUMBER_SPELLINGS = {
    "python": ("ordernumber",),
    "node": ("order_number", "either<f64, string>"),
    "c": ("quantity_text", "price_text"),
    "csharp": ("exactquantity", "exactprice"),
    "go": ("quantitytext", "pricetext"),
    "java": ("exactquantity", "exactprice"),
    "r": ("quantity_text", "price_text"),
}

AXIS_SPELLINGS = {
    "market": ("market_type", "markettype", "market"),
    "margin_mode": ("margin_mode", "marginmode"),
    "position_mode": ("position_mode", "positionmode"),
}


def constructor_params(label: str, source: str) -> str | None:
    """The exchange constructor's parameter list, or None if it cannot be found.

    Read from the raw source rather than the prose-stripped haystack: a
    parameter list carries no prose, and stripping `#` lines would take the C
    header's own declaration apart.
    """
    pattern = CONSTRUCTOR_SIGNATURE.get(label)
    if pattern is None:
        return None
    matches = [m.group(1) for m in re.finditer(pattern, source, re.S)]
    if not matches:
        return None
    return max(matches, key=len).lower()



def main() -> int:
    traits = trait_verbs()
    missing_traits = [t for t in REQUIRED_TRAITS if t not in traits]
    if missing_traits:
        print(f"traits.rs no longer declares {', '.join(missing_traits)}; "
              "update REQUIRED_TRAITS", file=sys.stderr)
        return 1

    contract: list[str] = []
    for trait in REQUIRED_TRAITS:
        for verb in traits[trait]:
            if verb not in contract:
                contract.append(verb)

    fields = order_fields()
    print(f"contract: {len(contract)} verbs across {len(REQUIRED_TRAITS)} traits "
          f"(read from {TRAITS}), {len(fields)} order fields "
          f"(read from {ORDER_REQUEST})\n")

    failures: list[str] = []
    for label, paths, spell in BINDINGS:
        source = read_all(*paths)
        if not source:
            failures.append(f"{label}: no source found at {', '.join(paths)}")
            print(f"  {label:<8} SOURCE MISSING")
            continue
        haystack = source.lower()
        absent = []
        for verb in contract:
            spellings = spell(verb)
            if not present(haystack, spellings):
                absent.append(f"{verb} (as {'/'.join(spellings)})")
        ctor_aliases = CONSTRUCTOR_ALIASES.get(label, {})
        ctor_absent = []
        for ctor in CONSTRUCTORS:
            spellings = ctor_aliases.get(ctor, (ctor,))
            if not present(haystack, spellings):
                ctor_absent.append(f"{ctor} (as {'/'.join(spellings)})")

        fields_absent = [
            f"{field} (as {'/'.join(ORDER_FIELD_SPELLINGS[field])})"
            for field in fields
            if not present(haystack, ORDER_FIELD_SPELLINGS[field])
        ]

        paths_absent = order_paths(label, haystack, source)

        exact_spellings = EXACT_NUMBER_SPELLINGS[label]
        exact_absent = (
            []
            if present(haystack, exact_spellings)
            else [f"an exact order number (as {'/'.join(exact_spellings)})"]
        )

        params = constructor_params(label, source)
        if params is None:
            config_absent = ["the exchange constructor could not be located"]
        else:
            config_absent = [
                f"{axis} (as {'/'.join(AXIS_SPELLINGS[axis])})"
                for axis in CONFIGURATION
                if not any(s in params for s in AXIS_SPELLINGS[axis])
            ]

        if absent or ctor_absent or config_absent or fields_absent or paths_absent \
                or exact_absent:
            detail = []
            if absent:
                detail.append("verbs: " + ", ".join(absent))
            if ctor_absent:
                detail.append("constructors: " + ", ".join(ctor_absent))
            if config_absent:
                detail.append("configuration: " + ", ".join(config_absent))
            if fields_absent:
                detail.append("order fields: " + ", ".join(fields_absent))
            if paths_absent:
                detail.append("order paths: " + ", ".join(paths_absent))
            if exact_absent:
                detail.append("exact numbers: " + ", ".join(exact_absent))
            failures.append(f"{label}: missing {'; '.join(detail)}")
            print(f"  {label:<8} {len(contract) - len(absent)}/{len(contract)} verbs, "
                  f"{len(CONSTRUCTORS) - len(ctor_absent)}/{len(CONSTRUCTORS)} constructors, "
                  f"{len(CONFIGURATION) - len(config_absent)}/{len(CONFIGURATION)} config, "
                  f"{len(fields) - len(fields_absent)}/{len(fields)} order fields, "
                  f"{len(ORDER_PATH_SPELLINGS) - len(paths_absent)}/{len(ORDER_PATH_SPELLINGS)} order paths, "
                  f"{1 - len(exact_absent)}/1 exact numbers"
                  f"  MISSING: {'; '.join(detail)}")
        else:
            print(f"  {label:<8} {len(contract)}/{len(contract)} verbs, "
                  f"{len(CONSTRUCTORS)}/{len(CONSTRUCTORS)} constructors, "
                  f"{len(CONFIGURATION)}/{len(CONFIGURATION)} config, "
                  f"{len(fields)}/{len(fields)} order fields, "
                  f"{len(ORDER_PATH_SPELLINGS)}/{len(ORDER_PATH_SPELLINGS)} order paths, "
                  f"1/1 exact numbers")

    if failures:
        print("\nbindings have drifted from the trait contract:", file=sys.stderr)
        for line in failures:
            print(f"  {line}", file=sys.stderr)
        return 1

    print(f"\nall {len(BINDINGS)} bindings carry the full contract.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
