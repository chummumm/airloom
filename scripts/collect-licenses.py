#!/usr/bin/env python3
"""Collect pinned dependency license texts for a binary distribution.

Reads Cargo metadata and the downloaded NuGet packages. It does not infer a new
license or rewrite third-party notices. Run after cargo fetch and nuget restore.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess


def license_files(directory: Path) -> list[Path]:
    candidates = []
    for child in directory.iterdir():
        if child.is_file() and child.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE")):
            candidates.append(child)
        elif child.is_dir() and child.name.lower() in ("license", "licenses", "legal"):
            candidates.extend(path for path in child.rglob("*") if path.is_file())
    return sorted(candidates)


def add_text(output: list[str], path: Path, base: Path) -> None:
    try:
        if path.stat().st_size > 2_000_000 or not path.resolve().is_relative_to(base.resolve()):
            return
        text = path.read_text(encoding="utf-8-sig")
    except (OSError, UnicodeError):
        return
    output.extend([f"--- {path.relative_to(base)} ---", text.strip(), ""])


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--cargo", default="cargo")
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    metadata = json.loads(subprocess.check_output(
        [args.cargo, "metadata", "--format-version", "1", "--locked", "--offline"],
        cwd=root,
        text=True,
        encoding="utf-8",
    ))
    output = ["AIRLOOM — THIRD-PARTY LICENSE TEXTS", "", "Generated from the exact packages in Cargo.lock and restored NuGet packages.", ""]
    missing = []
    packages = sorted((p for p in metadata["packages"] if p.get("source")), key=lambda p: (p["name"], p["version"]))
    for package in packages:
        directory = Path(package["manifest_path"]).parent
        output.extend(["=" * 72, f"{package['name']} {package['version']}", f"License: {package.get('license') or 'see included files'}", f"Source: {package.get('repository') or package.get('homepage') or package['source']}", ""])
        files = license_files(directory)
        if package.get("license_file"):
            explicit = directory / package["license_file"]
            if explicit.is_file() and explicit not in files:
                files.append(explicit)
        if not files:
            missing.append(f"{package['name']} {package['version']}")
        for path in files:
            add_text(output, path, directory)
    for packages_dir in (root / "native/Airloom/packages", root / "packages"):
        if not packages_dir.is_dir():
            continue
        for package_dir in sorted(path for path in packages_dir.iterdir() if path.is_dir()):
            files = license_files(package_dir)
            if not files:
                continue
            output.extend(["=" * 72, f"NuGet: {package_dir.name}", ""])
            for path in files:
                add_text(output, path, package_dir)
    if missing:
        raise SystemExit("Package archives missing license text: " + ", ".join(missing))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text("\n".join(output), encoding="utf-8")
    print(f"Collected license texts for {len(packages)} Rust packages: {args.output}")


if __name__ == "__main__":
    main()
