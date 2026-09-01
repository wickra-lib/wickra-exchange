#!/usr/bin/env python3
"""Assert that the R wrapper can link against the C ABI in this tree.

Every other binding is compiled from the same source tree in the same job, so a
wrapper and the ABI it calls cannot disagree. R is the exception: `src/Makevars`
takes the header and library from `WKEX_INC` / `WKEX_LIB`, supplied from
outside, and compiles the hand-written `src/wickra_exchange.c` against whatever
is there. Nothing in the R job proves that the wrapper's calls match the header
those variables point at -- CI happens to point them at the tree, so they match
by construction and the check never runs.

This makes the pairing explicit:

  * Every `wickra_*` symbol `src/wickra_exchange.c` calls must be declared in
    `bindings/c/include/wickra_exchange.h`. A call to a symbol that is not
    there is a link error waiting for whoever builds the package.
  * The argument count at each call site must match the declaration. This is the
    failure that hurts: a function that gains a parameter still links by name in
    a loose build and then reads a garbage argument.

Not yet checked, because the machinery does not exist here: skew against the ABI
of a *published* release. `wickra`'s R package downloads a prebuilt
`wickra-c-<triple>.tar.gz` for the version in `DESCRIPTION` through a `configure`
script, so its wrapper can be correct against the tree and wrong against the
release r-universe builds from. This package has no `configure` and is not
registered with r-universe; when both land, the second half of that check comes
with them.

Run from the repository root:  python scripts/check_r_abi_skew.py
"""

from __future__ import annotations

import glob
import os
import re
import sys

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
HEADER = "bindings/c/include/wickra_exchange.h"
WRAPPER_GLOB = "bindings/r/src/*.c"


def read(path: str) -> str:
    full = os.path.join(ROOT, path)
    if not os.path.isfile(full):
        raise SystemExit(f"{path} not found; run this from the repository root")
    with open(full, encoding="utf-8") as handle:
        return handle.read()


def strip_comments(text: str) -> str:
    text = re.sub(r"/\*.*?\*/", " ", text, flags=re.S)
    return re.sub(r"//[^\n]*", " ", text)


def declared_arities(header: str) -> dict[str, int]:
    """Symbol -> parameter count, from the header's function declarations."""
    header = strip_comments(header)
    arities: dict[str, int] = {}
    for match in re.finditer(r"\b(wickra_[a-z_0-9]+)\s*\(", header):
        name = match.group(1)
        params, depth, index = "", 1, match.end()
        while depth and index < len(header):
            char = header[index]
            depth += (char == "(") - (char == ")")
            if depth:
                params += char
            index += 1
        arities[name] = arity_of(params)
    return arities


def arity_of(params: str) -> int:
    params = params.strip()
    if not params or params == "void":
        return 0
    depth, count = 0, 1
    for char in params:
        if char in "([":
            depth += 1
        elif char in ")]":
            depth -= 1
        elif char == "," and depth == 0:
            count += 1
    return count


def call_sites(source: str) -> list[tuple[str, int, int]]:
    """(symbol, argument count, line) for every wickra_* call in the wrapper."""
    source = strip_comments(source)
    calls = []
    for match in re.finditer(r"\b(wickra_[a-z_0-9]+)\s*\(", source):
        name = match.group(1)
        args, depth, index = "", 1, match.end()
        while depth and index < len(source):
            char = source[index]
            depth += (char == "(") - (char == ")")
            if depth:
                args += char
            index += 1
        calls.append((name, arity_of(args), source.count("\n", 0, match.start()) + 1))
    return calls


def main() -> int:
    arities = declared_arities(read(HEADER))
    if not arities:
        raise SystemExit(f"no wickra_* declarations found in {HEADER}")

    wrappers = sorted(glob.glob(os.path.join(ROOT, WRAPPER_GLOB)))
    if not wrappers:
        raise SystemExit(f"no R wrapper sources at {WRAPPER_GLOB}")

    failures: list[str] = []
    called: set[str] = set()

    for path in wrappers:
        rel = os.path.relpath(path, ROOT).replace(os.sep, "/")
        with open(path, encoding="utf-8") as handle:
            source = handle.read()
        problems = 0
        for name, argc, line in call_sites(source):
            # The wrapper defines its own R-facing entry points; only calls into
            # the C ABI are the ABI's business.
            if name not in arities:
                if re.search(rf"(?m)^\s*(SEXP|static|void|int|double)[^\n]*\b{re.escape(name)}\s*\(",
                             source):
                    continue  # a function this file defines, not one it calls
                failures.append(f"{rel}:{line}: calls {name}, which {HEADER} does not declare")
                problems += 1
                continue
            called.add(name)
            if argc != arities[name]:
                failures.append(
                    f"{rel}:{line}: {name} called with {argc} argument(s); "
                    f"the header declares {arities[name]}")
                problems += 1
        status = "matches the header" if not problems else f"{problems} problem(s)"
        print(f"  {rel:<34} {status}")

    print(f"\n{len(called)} of {len(arities)} declared C ABI symbols are reached "
          "from the R wrapper.")

    if failures:
        print("\nthe R wrapper does not match the C ABI in this tree:", file=sys.stderr)
        for line in failures:
            print(f"  {line}", file=sys.stderr)
        print("\nregenerate or repair bindings/r/src, or update the header.",
              file=sys.stderr)
        return 1

    print("the R wrapper links cleanly against the header in this tree.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
