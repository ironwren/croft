#!/usr/bin/env python3
"""Release notes written in pull requests, versions assigned after merge.

Stub: the API scripts/tests/test_release.py drives, with no behaviour yet.
"""

from __future__ import annotations

import sys


class ReleaseError(Exception):
    """A cut that cannot go ahead."""


def check(base, head, cwd=None):
    return []


def cut(root, kind="patch"):
    return None


def main(argv):
    if argv[:1] == ["check"]:
        return 0
    if argv[:1] == ["cut"]:
        return 0
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
