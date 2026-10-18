#!/usr/bin/env python3
"""The workspace crates' versions and pyproject.toml's version must agree.

The Python package's version (pyproject.toml ``[project].version``) and every
workspace crate's ``[package].version`` are one shared version: the wheel, the
extension module and the crates that build it carry the same number, and
VERSIONING.md owns the bump rules. This check fails a tree where they
disagree, so a bump that misses a manifest cannot land.

Usage::

    check_version_sync.py              check this repository
    check_version_sync.py <root>       check the tree at <root>
    check_version_sync.py --self-test  run the check against a temporary
                                       agreeing fixture and a deliberately
                                       mismatched one, requiring the first
                                       to pass and the second to fail
"""

from __future__ import annotations

import sys
import tempfile
import tomllib
from pathlib import Path

FAIL_PREFIX = "FAIL: "


def fail(message: str) -> None:
    print(f"{FAIL_PREFIX}{message}", file=sys.stderr)


def read_pyproject_version(pyproject: Path) -> str:
    """The ``[project]`` table's version of a pyproject.toml."""
    document = tomllib.loads(pyproject.read_text())
    version = document.get("project", {}).get("version")
    if not isinstance(version, str) or not version:
        fail(f"no version under [project] in {pyproject}")
        raise SystemExit(1)
    return version


def read_workspace_members(root: Path) -> list[str]:
    """The member paths of the workspace whose manifest sits at ``root``."""
    with (root / "Cargo.toml").open("rb") as handle:
        document = tomllib.load(handle)
    members = document.get("workspace", {}).get("members", [])
    paths = [member for member in members if isinstance(member, str)]
    if not paths:
        fail(f"could not read the workspace member list from {root / 'Cargo.toml'}")
        raise SystemExit(1)
    return paths


def read_crate_version(manifest: Path) -> str:
    """The ``[package]`` table's version of a Cargo.toml."""
    document = tomllib.loads(manifest.read_text())
    version = document.get("package", {}).get("version")
    if not isinstance(version, str) or not version:
        fail(f"{manifest} declares no [package] version")
        raise SystemExit(1)
    return version


def check(root: Path) -> bool:
    """Whether every workspace crate's version equals pyproject.toml's.

    Prints one line per crate and a verdict; False names each crate that
    disagrees on stderr.
    """
    pyproject = root / "pyproject.toml"
    if not pyproject.is_file():
        fail(f"{pyproject} does not exist")
        return False
    cargo_root = root / "Cargo.toml"
    if not cargo_root.is_file():
        fail(f"{cargo_root} does not exist")
        return False

    py_version = read_pyproject_version(pyproject)
    print(f"pyproject.toml: {py_version}")

    ok = True
    for member in read_workspace_members(root):
        manifest = root / member / "Cargo.toml"
        if not manifest.is_file():
            fail(f"workspace member {member} has no Cargo.toml")
            return False
        version = read_crate_version(manifest)
        if version == py_version:
            print(f"  ok: {member} {version}")
        else:
            print(
                f"  MISMATCH: {member} is {version}, pyproject.toml is {py_version}",
                file=sys.stderr,
            )
            ok = False

    if not ok:
        fail(
            f"workspace crate versions disagree with pyproject.toml's "
            f"{py_version} (VERSIONING.md owns the bump rules)"
        )
        return False
    print(
        f"OK: every workspace crate version agrees with pyproject.toml ({py_version})"
    )
    return True


def write_workspace(root: Path, crate_one: str, crate_two: str, pyproject: str) -> None:
    """A minimal two-crate workspace plus pyproject.toml at ``root``."""
    (root / "crates" / "one").mkdir(parents=True)
    (root / "crates" / "two").mkdir(parents=True)
    (root / "Cargo.toml").write_text(
        '[workspace]\nmembers = [\n    "crates/one",\n    "crates/two",\n]\n'
    )
    (root / "crates" / "one" / "Cargo.toml").write_text(
        f'[package]\nversion = "{crate_one}"\n'
    )
    (root / "crates" / "two" / "Cargo.toml").write_text(
        f'[package]\nversion = "{crate_two}"\n'
    )
    (root / "pyproject.toml").write_text(f'[project]\nversion = "{pyproject}"\n')


def self_test() -> bool:
    """An agreeing fixture passes; one crate that missed the bump fails, named."""
    with tempfile.TemporaryDirectory(prefix="version-sync-selftest-") as tmp:
        base = Path(tmp)

        agree = base / "agree"
        write_workspace(agree, "0.1.0", "0.1.0", "0.1.0")
        if not check(agree):
            fail("self-test: the agreeing fixture must pass")
            return False

        disagree = base / "disagree"
        write_workspace(disagree, "0.2.0", "0.1.0", "0.2.0")
        if check(disagree):
            fail("self-test: the mismatched fixture must fail")
            return False

    print(
        "OK: self-test passed (the agreeing fixture passes, the mismatched fixture fails naming the crate)"
    )
    return True


def main(argv: list[str]) -> int:
    if len(argv) > 1:
        print(__doc__ or "", file=sys.stderr)
        return 2
    if argv and argv[0] == "--self-test":
        return 0 if self_test() else 1
    root = Path(argv[0]).resolve() if argv else Path(__file__).resolve().parent.parent
    return 0 if check(root) else 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
