---
id: I-07-T4
title: "python_to_json_value f64-arm comment misattributes the huge-int rejection mechanism"
status: done
created_at: 2026-09-25
created_by: i07-coverage-triage
resolved_at: 2026-09-25
resolved_by: i07-fix-loop
---

## Resolution (fix loop, 2026-09-25)

Settled by execution, not deferral: the comment at the f64 arm now
states the verified mechanism — genuine non-finite floats
(`float("nan")`, ±inf) are rejected by this arm's `is_finite` check,
while Python ints too large for i64 (e.g. 10**400) fail pyo3's f64
extraction in practice and fall through to the unsupported-type
catch-all, which rejects them loudly there — both layers stay loud.
Both mechanisms re-verified by execution through the real channel
(`overrides`): the huge int raises
`ValueError: cannot convert Python object of type 'int' to JSON`
(the catch-all) and the NaN raises
`ValueError: non-finite float value (NaN) cannot cross the Python
boundary ...` (the `is_finite` arm) — the corrected comment's claims
are pinned by measurement. The huge-int loudness face remains gated by
`test_non_finite_overrides_value_fails_loudly`
(`tests/python/test_py_config.py`, layer unpinned by design).

# Comment misattributes which layer rejects huge Python ints

## Problem

`crates/hares-python/src/utils.rs`, the f64 arm of
`python_to_json_value` (the unified Python→JSON boundary converter
landed in I-07's boundary fix), carries the comment:

> Also catches Python ints too large for i64 that extract as infinite
> f64 (e.g. 10**400).

The coverage phase (round 14) verified by execution that this mechanism
claim is wrong for the current pyo3 version: pyo3's f64 extraction of
`10**400` *fails* rather than yielding `inf`, so the value is actually
rejected by the converter's unsupported-type catch-all ("cannot convert
Python object of type 'int' to JSON"). The `is_finite` arm the comment
annotates never sees such ints.

The observable contract is correct and gated — gate 19
(`test_non_finite_overrides_value_fails_loudly`,
`tests/python/test_py_config.py`) pins that `10**400` raises loudly,
layer unpinned — so no behavioral defect exists and no failing test can
be written against this. What is wrong is only the comment's mechanism
claim, which fails the constitution's commentary bar (a reader must be
able to understand the reasoning from the comment alone; this one
describes a code path that does not execute for the named case).

## What goes wrong if unaddressed

A maintainer reasoning about arm coverage from the comment believes the
`is_finite` arm covers huge ints and could weaken the catch-all (or
reorder arms) on that basis. The gates pin the loud contract, so a
silent regression would still go red — the residual is documentation
drift in boundary code where the loud-error contract lives, and the
analysis the coverage phase already did evaporates if not tracked.

## Suggested fix

One comment correction at the f64 arm, e.g.: note that ints too large
for i64 fail pyo3's f64 extraction in practice and fall to the
catch-all, while this arm's `is_finite` check is what rejects genuine
f64 NaN/±inf (`float("nan")`, `float("inf")`) — keep both layers loud.

## Why a ticket rather than a flow cycle

Triage could not route this as `defect_found`: the schema requires a
genuinely failing gate command, and no behavioral failure exists to
prove (the contract is correct and pinned). It is a comment-accuracy
item in code this initiative introduced — below the flow-cycle bar,
above the evaporate-it bar.
