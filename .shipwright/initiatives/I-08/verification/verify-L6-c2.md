# Verification L6-c2: consumer ledger CN-1 to CN-15, full depth

Scope: `ledger/08-consumer.md`, every citation in CN-1 through CN-15 and in
CN-17's citations that point into the consumer repository (CN-0, CN-16,
B0, B0 re-check and CN-17 itself were read for context and cross-checked
where they bear on CN-1 to CN-15). Citations into the consumer were read
with `git -C ~/src/HARES_CALIBRATION_SYSTEM show 5c651bc:<path>` against
branch `i08-consumer` (HEAD `5c651bc`); citations into the engine were
read against `/home/rich/src/HARES/.workstrees/audit-7b7a6a7d` (HEAD
`7b7a6a7d`) when unpinned, or at the commit the entry names when pinned.

Entries read in full: CN-0, CN-16, B0, CN-17, B0 re-check, CN-1 through
CN-15 (every line of the ledger file, 2092 lines).

Verification performed:
- Extracted every cited consumer file at `5c651bc` (`engine.py`,
  `parallel.py`, `fit.py`, `scale.py`, `inject.py`, `registry.py`,
  `coverage.py`, `paths.py`, `hares_contract.py`, `hares_contract_check.py`,
  `capture_de_trace.py`, `build_bundle.py`, `check_losses.py`,
  `forward_model.py`, `defaults.py`, `der_verify.py`,
  `tests/suite_ami2.py`) and checked cited line ranges against cited
  content, line by line for the heavily-cited files.
- Diffed every consumer file between `c71c542` (the ledger's stated
  evidence commit) and `5c651bc` (HEAD) to find every file the ledger's
  "at `c71c542` unless named" convention could have gone stale against:
  `build_bundle.py`, `calibration/der_verify.py`, `calibration/scale.py`,
  `calibration/tests/suite_ami2.py`, `calibration/tests/suite_scale.py`,
  `pyproject.toml`, `tools/capture_de_trace.py`, `uv.lock`. Everything
  else CN-1 to CN-15 cite is unchanged between the two commits, so no
  other citation in this scope can have drifted on line number.
- For each of those changed files, diffed old against new content and
  confirmed whether any CN-1-to-CN-15 citation crossed an inserted/deleted
  region.
- Ran the completeness greps the task named (and the superset the
  entries' own census commands specify) against the consumer tree at
  `5c651bc`: `write_output`, `output_verbosity`, `output_to_parquet`,
  `output_path`, `resample_overrides`/`RESAMPLE_OVERRIDES`,
  `finalize_billing`, `.metrics()`, `.results()`, `Runner(` (every
  construction site), and a full case-insensitive `ochre[_-]next` sweep,
  and matched every hit against CN-1's and CN-13's census lists.
- Checked the engine-side citations pinned to `0ec757cb` (CN-16's
  equipment-identity lines) and the unpinned ones read at `7b7a6a7d`
  (CN-3's envelope-LUT precedence file), by exact line number.
- Checked the `uv`/`ruff` CLI flags the mechanism and CN-12/CN-15 use
  (`--no-install-package`, `--locked`, `--dry-run`, `--build-constraints`,
  `--reinstall-package`, `--editable`) against the installed `uv 0.12.8`
  `--help` output; all exist as described.
- Confirmed `pytest` is not a consumer dependency at `5c651bc`
  (`pyproject.toml`: only `ruff` in `[dependency-groups] dev`), matching
  the gate's "pytest is not a consumer dependency" claim.

Not independently re-run: the consumer gate, the DE-trace capture/compare,
or any engine build (none of CN-1 to CN-15 has landed; there is no commit
to gate). The B-series numeric-fix files CN-3/CN-4 depend on were not
re-verified here (out of this pass's scope; `09-numeric-fixes.md` is
covered by `verify-L6-c.md`).

## Findings

### Major: CN-1's consumer evidence/acceptance citations into `suite_ami2.py` are stale by 41 lines for every citation past the pre-CN-16 line 1502

Entry/evidence: `ledger/08-consumer.md` CN-1, four places:
- Evidence, the direct-construction-sites bullet: "the tests
  `suite_ami2.py:813,862,1711,1929-1931`."
- Evidence, the global-verbosity bullet: "`suite_ami2.py:54-63, 400-411,
  882-888, 1182-1189, 1302-1306, 1730-1735, 1923-1937, 2021-2025,
  2081-2090, 2146-2156`."
- Acceptance test census: "`suite_ami2.py:61,406,809,857,1186,1253,1298,
  1708,1931,2086,2152`".
- Change, the text-migration bullet: "`suite_ami2.py:813, 862, 1711,
  1930`".

Root cause, confirmed by diff: CN-16 landed (`a6d1345`) after CN-1's
census was written but before the consumer's current HEAD (`5c651bc`).
CN-16's commit inserts content into `suite_ami2.py` in two hunks ending at
old line 1502: a 1-line-to-9-line replacement at old line 1441 (net +8)
and a new 33-line test case after old line 1502 (net +33), for a
cumulative +41 shift on every line after old 1502. Lines at or before old
1441 are unaffected.

Verified line-by-line: every citation at or below 1441/862/813 etc. still
points at the content described (`str(spec.hpxml)` construction calls,
`Runner(...)` constructions, `OUTPUT_VERBOSITY` save/restore blocks).
Every citation above old line 1502 now points at unrelated code at
`5c651bc` (a GridLAB-D energy-balance assertion, an `f8_phase` voltage
check, etc.), while the actual content described has moved 41 lines
later. Confirmed by direct content comparison for every one of the
affected ranges (not sampled): `1711`->`1752`, `1929-1931`->`1970-1972`,
`1708`->`1749`, `1931`->`1972`, `2086`->`2127`, `2152`->`2193`,
`1930`->`1971`, `1730-1735`->`1771-1776`, `1923-1937`->`1964-1978`,
`2021-2025`->`2062-2066`, `2081-2090`->`2122-2131`,
`2146-2156`->`2187-2197`.

Contrast: CN-3's own citations into the same file (`suite_ami2.py:822,
879, 1767` and `:2016`) already use the post-CN-16, `5c651bc`-correct
line numbers, so this is not a convention mismatch across the whole
document — CN-1's census specifically was never updated after CN-16
landed underneath it.

Fix applied: edited all four locations in `ledger/08-consumer.md` to the
`5c651bc`-correct line numbers, each annotated `(at 5c651bc)` or with a
short note on the shift, so a future reader is not misled by a silent
renumbering and so the convention (evidence nominally at `c71c542`) is
flagged as overridden for this specific citation set.

### Confirmed correct (high confidence)

- CN-0's registrations (`tools/capture_de_trace.py`, `tools/check_losses.py`,
  `tools/race_gridlabd.py`, `docs/Red_Team_Record.md` in `build_bundle.py`
  `SOURCE`) are present at `5c651bc`; `HomeCalibration.x` and its fill from
  `r.x` (`scale.py:497-499`, `:583`) are present, confirming CN-0 landed as
  described.
- CN-16's fix is landed and matches its own Problem/Change exactly:
  `der_verify.py` at `5c651bc` already states the measured symptom without
  naming a cause; `suite_ami2.py` already fails fast on `det is None`
  before any TypeError-prone use. The engine identity-assignment citation
  (`dwelling/mod.rs:1477, 2414, 3490` at `0ec757cb`) is exact to the line.
- CN-9's problem citations (`check_losses.py:24-26` PINNED four-decimal
  values, `:34` the rounding, `:44` unconditional `return 0`) match
  `5c651bc` exactly; CN-9 has not landed, consistent with the schedule.
- CN-13's `ochre_next`/`ochre-next` census is complete: a full
  case-insensitive sweep of the consumer tree at `5c651bc` over every path
  the entry's own census command names returns 53 hits, and every one of
  them is accounted for in either the "Evidence" import list or the "Text
  naming it" list (including the one line explicitly carved out as a
  dated record, `docs/HARES_Calibration_Code_and_Verification.md:524`).
  No missed call site.
- CN-1's `Runner(` construction-site census (the non-test "Engine-output
  scratch" bullet) is complete against a full-tree grep: every production
  (non-test) `Runner(` call site at `5c651bc` is named.
- `write_output`, `output_verbosity`, `output_to_parquet`, `output_path`
  grep hits at `5c651bc` match CN-1's Evidence list exactly, including the
  `forward_model.py`, `hares_der_remeasure.py`, `hares_l1_archetype_sweep.py`
  and `hares_ua_xval.py` sites, and `schema.py`'s `Prober` construction.
- `resample_overrides`/`RESAMPLE_OVERRIDES` and `finalize_billing` grep
  hits at `5c651bc` match CN-1's citations exactly (the latter: only
  `ami2/tariff.py:79` docstring and `:90` the call, both cited).
- CN-3's envelope engine citation
  (`crates/hares-core/src/dwelling/conversions.rs:147-151, 240-291`) is
  confirmed in the audit worktree: the cited lines are the
  `AssemblyEffectiveRValue`-vs-LUT material/R-sum fallback and the LUT
  lookup path the entry describes.
- CN-17's `capture_de_trace.py` citations (empty `RULES` dict, the
  `cmd_capture` provenance guard reading `paths.HARES_REPO` and
  `summary()["engine_so"]`, `compare` having no `--declared` option yet)
  all match `5c651bc` exactly.
- The `uv`/`uv pip` flags the engine-under-test mechanism and CN-12/CN-15
  use (`--no-install-package`, `--locked`, `--dry-run`,
  `--build-constraints`, `--reinstall-package`, `--editable`) all exist in
  the installed `uv 0.12.8`. The consumer has no `pytest` dependency at
  `5c651bc` (only `ruff` in the `dev` group), matching the gate's claim
  that pytest is not a consumer dependency.

## Residual risk / scope not completed

- The fifteen entries' citations number in the hundreds; this pass
  verified every citation into the eight consumer files that actually
  changed between `c71c542` and `5c651bc` (where staleness is possible)
  plus a representative deep sample of the unchanged files' citations
  (`engine.py`, `registry.py`, `defaults.py`, `coverage.py`,
  `capture_de_trace.py`). Citations into consumer files that did not
  change between `c71c542` and `5c651bc` cannot have drifted on line
  number by construction, so they were spot-checked rather than
  exhaustively re-counted character-for-character.
- CN-2, CN-4 through CN-12, CN-14 and CN-15 were read in full and their
  dependency chains and gate/verification text cross-checked for internal
  consistency, but their cited consumer lines fall in files that did not
  change since `c71c542` (`fit.py`, `parallel.py`, `coverage.py`,
  `registry.py`, `paths.py`), so no drift was possible there; they were
  not line-by-line diffed against `c71c542` beyond the samples reported
  above, since `c71c542`-vs-`5c651bc` identity is already established at
  the whole-file level.
- The consumer gate and DE-trace gate were not run (no entry in this
  range has landed; there is nothing to gate).

## Summary

Entries read: CN-0, CN-16, B0, B0 re-check, CN-17 (context) + CN-1 through
CN-15 (full scope), all of `08-consumer.md`.

Findings: 1 major, 0 blockers, 0 minor beyond the one major.

- Major: CN-1's evidence and acceptance-test citations into
  `calibration/tests/suite_ami2.py` were stale by 41 lines for every
  citation past old line 1502, because CN-16 inserted content into that
  file after CN-1's census was written. Four locations, confirmed line by
  line against both `c71c542` and `5c651bc`, fixed in
  `ledger/08-consumer.md` to the current (`5c651bc`) line numbers.

No other citation checked in this pass pointed at the wrong content, and
no missed consumer call site was found for `simulate`, `results`,
`metrics`, `write_output`, `output_verbosity`, `Runner.scratch`,
`contract`, `ochre_next`, `finalize_billing` or `resample_overrides`:
every grep hit against the consumer tree at `5c651bc` for these names is
accounted for in the owning entry's census.
