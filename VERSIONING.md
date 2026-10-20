# Versioning

HARES is pre-1.0 and versions as `0.M.P`: a breaking change to any
surface below bumps `M`, an addition or a fix bumps `P`. One version
number is shared: the Python package's (`pyproject.toml`
`[project].version`) and every workspace crate's (`[package].version`)
move together in the same commit. `scripts/check_version_sync.py` fails
CI when the manifests disagree (and
`tests/python/test_version_sync.py` runs the same check, plus a
deliberately mismatched fixture, in the pytest suite). Every bump adds
its section to [CHANGELOG.md](CHANGELOG.md).

Three surfaces carry a semver promise to consumers:

## The typed Python surface

`python/hares/_hares.pyi` and the pure-Python package that re-exports
it (`python/hares/`) are the API every consumer imports. Removing or
renaming a class, method, keyword, enum member or catalogue variable,
changing a parameter's requiredness or type, or changing the exception
type a call raises is breaking (bump `M`). Adding a class, method or
keyword with a default, an enum member or an optional field is additive
(bump `P`).

## The Gymnasium API

`hares.rl` (`DwellingGymEnv`, `VecDwellingGymEnv`) promises Gymnasium's
`Env` contract: the declared observation and action spaces (shapes,
bounds, dtypes), the reset/step lifecycle and its return types, the
`info` keys' names and types, and the termination and truncation
semantics of an episode. Changing any of those is breaking (bump `M`);
adding an optional constructor keyword or a new `info` entry is
additive (bump `P`).

## The checkpoint format

A checkpoint written by any build of a version restores bitwise in
every later build of the same `M` (the postcard + CRC records over the
dwelling's state). Any change to the serialized bytes (a new field, a
reordering, a type change) is breaking even when it is additive in the
Rust sense: a checkpoint is data leaving the process. A format change
re-captures the checkpoint goldens with its cause recorded and bumps
`M`.

## Who bumps what when

The developer who makes the change bumps, in the change's own commit,
before it lands on the mainline branch:

- Breaking change to one of the three surfaces: bump `M` in
  `pyproject.toml` and in every workspace crate's `Cargo.toml` (the
  check script names any manifest that missed it), and add the
  breaking change with its replacement to `CHANGELOG.md`.
- Additive change or fix: bump `P` the same way, and add the
  changelog entry.
- A change inside a surface that no consumer can observe (a doc
  comment, an internal refactor) bumps nothing.

A submodule re-pin is not a version bump: it changes no surface's
contract. Its procedure lives in
[docs/submodules.md](docs/submodules.md).
