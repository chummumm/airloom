#!/usr/bin/env python3
"""Enforce Airloom's release sequence: patch 0..10, then increment minor."""

import argparse
import re
import unittest
from pathlib import Path
import tomllib


def parse_version(value: str) -> tuple[int, int, int]:
    match = re.fullmatch(r"v?(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", value)
    if match is None:
        raise ValueError(f"Expected a stable three-part version, got {value!r}")
    return tuple(int(part) for part in match.groups())


def next_version(previous: str) -> tuple[int, int, int]:
    major, minor, patch = parse_version(previous)
    if patch > 10:
        raise ValueError("The previous release is outside the patch 0..10 policy")
    return (major, minor + 1, 0) if patch == 10 else (major, minor, patch + 1)


def verify(previous: str, proposed: str) -> None:
    expected = next_version(previous)
    actual = parse_version(proposed)
    if actual != expected:
        expected_text = ".".join(map(str, expected))
        raise ValueError(f"After {previous}, the next release must be {expected_text}; got {proposed}")


def check_files() -> None:
    root = Path(__file__).resolve().parent.parent
    with (root / "Cargo.toml").open("rb") as source:
        version = tomllib.load(source)["workspace"]["package"]["version"]
    numbers = parse_version(version)
    checks = {
        "installer/Airloom.iss": [f'#define AppVersion "{version}"'],
        "native/Airloom/Updater.h": [f'currentVersion{{L"{version}"}}'],
        "native/Airloom/Updater.cpp": [f'L"Airloom/{version}"'],
        "native/Airloom/app.manifest": [f'assemblyIdentity version="{version}.0"'],
        "native/Airloom/Airloom.rc": [
            'FILEVERSION ' + ','.join(map(str, (*numbers, 0))),
            'PRODUCTVERSION ' + ','.join(map(str, (*numbers, 0))),
            f'VALUE "FileVersion", "{version}.0\\0"',
            f'VALUE "ProductVersion", "{version}\\0"',
        ],
    }
    for filename, expected in checks.items():
        contents = (root / filename).read_text(encoding="utf-8")
        for value in expected:
            if value not in contents:
                raise ValueError(f"{filename} does not match Cargo.toml version {version}")
    print(f"Application, installer and update versions match {version}")


class ReleaseSequenceTests(unittest.TestCase):
    def test_successor(self):
        for previous, proposed in (("v0.4.0", "0.4.1"), ("0.4.8", "0.4.9"),
                                   ("0.4.9", "0.4.10"), ("0.4.10", "0.5.0"),
                                   ("0.9.10", "0.10.0")):
            with self.subTest(previous=previous):
                verify(previous, proposed)

    def test_rejects_skips_repeats_and_downgrades(self):
        for previous, proposed in (("0.4.0", "0.5.0"), ("0.4.0", "0.4.2"),
                                   ("0.4.9", "0.5.0"), ("0.4.10", "0.4.11"),
                                   ("0.4.1", "0.4.1"), ("0.4.1", "0.3.10")):
            with self.subTest(previous=previous, proposed=proposed):
                with self.assertRaises(ValueError):
                    verify(previous, proposed)

    def test_rejects_malformed_or_preview(self):
        for value in ("0.4", "0.4.1.0", "0.04.1", "0.4.1-preview.1", "-1.4.1", "0.4.1\n"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                parse_version(value)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("previous", nargs="?")
    parser.add_argument("proposed", nargs="?")
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--check-files", action="store_true")
    args = parser.parse_args()
    if args.check_files:
        try:
            check_files()
        except ValueError as error:
            parser.exit(1, f"Version consistency rejected: {error}\n")
        return 0
    if args.self_test:
        result = unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(ReleaseSequenceTests))
        return 0 if result.wasSuccessful() else 1
    if args.previous is None or args.proposed is None:
        parser.error("previous and proposed versions are required")
    try:
        verify(args.previous, args.proposed)
    except ValueError as error:
        parser.exit(1, f"Release version rejected: {error}\n")
    print(f"Release sequence verified: {args.previous} -> {args.proposed}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
