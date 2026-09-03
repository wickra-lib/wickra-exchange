#!/usr/bin/env python3
"""Assert that the test count the README claims is the test count that exists.

`README.md` advertises how many unit tests `wickra-exchange-core` carries. That
number is maintained by hand, and a hand-maintained number in a README drifts
the moment anyone adds a test and does not think of it. It has now drifted
three times: 441 while there were 513, then 534 while there were 540. Each time
it was found by someone counting, not by anything failing.

Nothing else can catch it. The suite passing says nothing about a sentence in a
different file, and a reader has no way to tell a stale number from a current
one -- it is a claim about the project's own thoroughness, which is exactly the
kind of claim that should not be taken on trust.

So the number is derived here instead. `#[test]` and `#[tokio::test]` in the
crate's `src/` are what `cargo test -p wickra-exchange-core --lib` runs, and the
two agree exactly: this counts the attributes rather than parsing cargo's
output, so the check needs no toolchain and runs in the same job as its five
siblings.

Run from the repository root:  python scripts/check_test_count.py
"""

from __future__ import annotations

import os
import re
import sys

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
CRATE = "crates/wickra-exchange-core/src"
README = "README.md"

# The sentence under audit. The number is the one capture; everything around it
# is matched so a differently-worded claim elsewhere cannot be mistaken for it.
CLAIM = re.compile(r"`wickra-exchange-core`\*{0,2} — (\d+) unit tests")

# Both spellings cargo collects into the lib test binary. `#[ignore]`d tests are
# counted too: cargo reports them in the same total, as `N ignored`.
TEST_ATTRIBUTE = re.compile(r"#\[(?:tokio::)?test\]")


def count_tests() -> int:
    total = 0
    for base, _, names in os.walk(os.path.join(ROOT, CRATE)):
        for name in sorted(names):
            if not name.endswith(".rs"):
                continue
            with open(os.path.join(base, name), encoding="utf-8") as handle:
                total += len(TEST_ATTRIBUTE.findall(handle.read()))
    return total


def main() -> int:
    actual = count_tests()

    path = os.path.join(ROOT, README)
    with open(path, encoding="utf-8") as handle:
        readme = handle.read()

    match = CLAIM.search(readme)
    if match is None:
        print(
            f"{README}: could not find the unit-test claim. If it was reworded, "
            "update CLAIM in scripts/check_test_count.py so it stays checked "
            "rather than quietly unchecked.",
            file=sys.stderr,
        )
        return 1

    claimed = int(match.group(1))
    line = readme[: match.start()].count("\n") + 1

    if claimed != actual:
        print(
            f"{README}:{line} claims {claimed} unit tests; {CRATE} has {actual}.",
            file=sys.stderr,
        )
        print(f"  fix: change {claimed} to {actual} in {README}.", file=sys.stderr)
        return 1

    print(f"{README}:{line} claims {claimed} unit tests, and {CRATE} has {actual}.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
