# Row 97 (CN-4): the re-baseline proposal, pending the operator's word

Written 2026-10-16, before rows 86-94 land. NOTHING here runs without the
operator's approval.

## The finding

Row 74 (CN-9) made `tools/check_losses.py` compare the calibration losses
exactly (float.hex, no tolerance). On the first honest run it went red:
every one of the 13 premises' losses sits 2 to 15 percent above its B0
pin. The movement is bitwise-reproducible across independent runs, serial
equals concurrent, and the cause is bracketed to the consumer span
`aec2e3f..cf7f2c33`: the strict-input repair's nine re-expressed HPXML
documents and the defaults-tree moves. Rows 72 (the report API) and 73
(the pool sizing) are exonerated by direct measurement; the DE-trace
production set bypasses the repair and stayed bitwise identical.

On top of that, the engine's weather and warm-up rows (79-81, 83-84)
declared real movement: the interpolation conventions, the calendar
addressing and the EnergyPlus warm-up criteria change the initial
conditions, so the fit's parameters, scores and bound flags move again
(the consumer's two F7/GEB gate failures; 8 new B0 paths).

## The ledger's own rule

`08-consumer.md`'s preamble: the consumer re-baselines ONCE, after all
engine numeric fixes (B1, CN-4). Row 97's commit message form is
recorded: `fix: re-pin the calibration loss regression to the corrected
engine`. Rows 87-94 (B-6, B-11, B-7, B-8, B-9, B-10, B-12, B-13) are the
numeric block's tail and land before it.

## What the re-baseline must contain when it runs

1. The full attribution: every loss's before/after with the deltas, the
   cause families named per the plan's rule (the strict-input repair's
   document re-expressions; the weather interpolation; the warm-up
   criteria; the calendar addressing), each with its row's citation.
2. The capture on the exact folded engine head, twice, byte-identical,
   with the header provenance (engine commit, consumer commit, lock
   sha256).
3. The new B0-losses.json + B1 pin written beside the old ones (the old
   pins kept, never rewritten: the record's own rule).
4. The row's State in `00-schedule.md` + the measurement log's entry.

## The decision

The operator's word starts row 97. Until then the two consumer gate
failures (F7/GEB) and the B0 deltas stay as recorded, owned by this row,
not silently re-pinned by any agent.
