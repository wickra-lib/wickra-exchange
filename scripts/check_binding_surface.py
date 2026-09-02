#!/usr/bin/env python3
"""Assert that every binding exposes the same API surface.

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
    """
    text = re.sub(r"/\*.*?\*/", " ", text, flags=re.S)
    text = re.sub(r"(?m)//[^\n]*", " ", text)
    text = re.sub(r"(?m)#[^\n]*", " ", text)
    return re.sub(r'"(?:\\.|[^"\\])*"', ' "" ', text)


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

    print(f"contract: {len(contract)} verbs across {len(REQUIRED_TRAITS)} traits "
          f"(read from {TRAITS})\n")

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

        if absent or ctor_absent:
            detail = []
            if absent:
                detail.append("verbs: " + ", ".join(absent))
            if ctor_absent:
                detail.append("constructors: " + ", ".join(ctor_absent))
            failures.append(f"{label}: missing {'; '.join(detail)}")
            print(f"  {label:<8} {len(contract) - len(absent)}/{len(contract)} verbs, "
                  f"{len(CONSTRUCTORS) - len(ctor_absent)}/{len(CONSTRUCTORS)} constructors"
                  f"  MISSING: {'; '.join(detail)}")
        else:
            print(f"  {label:<8} {len(contract)}/{len(contract)} verbs, "
                  f"{len(CONSTRUCTORS)}/{len(CONSTRUCTORS)} constructors")

    if failures:
        print("\nbindings have drifted from the trait contract:", file=sys.stderr)
        for line in failures:
            print(f"  {line}", file=sys.stderr)
        return 1

    print(f"\nall {len(BINDINGS)} bindings carry the full contract.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
