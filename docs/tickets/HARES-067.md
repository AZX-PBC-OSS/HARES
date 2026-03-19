---
id: HARES-067
title: "ochre_next — nested_update Override Semantics"
kind: implement
depends_on: [HARES-051]
files_to_touch:
  - python/ochre_next/utils.py
  - python/ochre_next/dwelling.py
references:
  - docs/architecture/09-roadmap.md
  - ochre/utils/base.py (upstream reference)
verification:
  - uv run pytest tests/python/ -v -k "nested_update"
---

## Background/Context

OCHRE uses `nested_update(d_old, d_new)` (a recursive dict merger from `ochre/utils/base.py`) to apply user kwargs (Equipment, Envelope, Occupancy dicts) as deep overrides on top of HPXML-derived defaults. This is listed as a Phase 2 deliverable in `09-roadmap.md`. The compat layer (HARES-051) exposes `Dwelling.__init__` but without recursive override semantics, shallow dict assignment clobbers HPXML-derived sub-keys that the caller did not intend to override.

## Work to Do

- [ ] Implement `nested_update(d_old: dict, d_new: dict) -> dict` in `python/ochre_next/utils.py`:
  - [ ] For each key in `d_new`: if both `d_old[key]` and `d_new[key]` are `Mapping`, recurse; otherwise overwrite `d_old[key]` with `d_new[key]`
  - [ ] Return a new dict (do not mutate `d_old`)
  - [ ] Handle missing keys in `d_old` — treat as empty dict for recursion
- [ ] Apply `nested_update` in `Dwelling.__init__` for the `Equipment`, `Envelope`, and `Occupancy` kwargs:
  - [ ] Merge each kwarg over the HPXML-derived defaults using `nested_update`
  - [ ] Empty override dict must be a strict no-op (HPXML defaults unchanged)

## Measures of Success

- [ ] `nested_update({"a": {"b": 1, "c": 2}}, {"a": {"b": 99}})` returns `{"a": {"b": 99, "c": 2}}`
- [ ] `nested_update({"x": 1}, {"x": 2, "y": 3})` returns `{"x": 2, "y": 3}`
- [ ] `nested_update({"a": {"b": 1}}, {})` returns `{"a": {"b": 1}}` (no-op)
- [ ] `Dwelling(hpxml_file=..., Equipment={"Battery": {"capacity_kwh": 20}})` overrides only the specified battery parameter, preserving HPXML defaults for all other battery fields and all other equipment
- [ ] Empty override dict `{}` passed as `Equipment={}` leaves HPXML defaults unchanged

## Verification

- [ ] `uv run pytest tests/python/ -v -k "nested_update"` passes
- [ ] `uv run ruff check python/ochre_next/utils.py` passes
