# Decision Log

## dl-d354d52a-bf1b-4c3c-98d4-46b865b516c1

```yaml
event_id: dl-d354d52a-bf1b-4c3c-98d4-46b865b516c1
timestamp: 2026-09-14T17:08:49.142Z
actor: operator
action: flow_recipe_selected
target: Z:/home/rich/src/HARES/.shipwright/flows/fix.yaml
recipe_sha256: 857ab6a3ed3c0e283c8ef77f2f596cbde15393369c2a69347be7f0fb8b88b778
reason: workspace_override
```

## dl-fix-372a3d70-operator-proceed-decision

fix-plan.md revision 9 set `plan_outcome: blocked`, putting one question to
the operator: proceed with Fixes 1, 2, 3, 5, 6 now (closing the confirmed
silence-contract root cause and all four `reproduce` gate tests) with the
ticket's literal `LowSoc 0.15` configuration tracked in
`docs/tickets/132-ev-hpxml-element-non-standard-anxiety-band-hardcoded.md`,
or hold I-05 until ticket 132 is diagnosed and resolved first.

The operator launched the `v2-fix-372a3d70-…` fix run against that plan —
the decision to **proceed**. The fix landed Fixes 1, 2, 3, 5, 6 together
plus two deviations recorded in the ticket's `## Implementation Notes →
### Deviations` (the `resolve()` target-selection rule extended beyond the
plan's tie-break so a targetless rate winner cannot erase a configured
ceiling — without it the plan's own W4 claim was false for `TouAware`; and
`build_actors_from_seeds` made `pub(crate)` for the integration tests). The
`LowSoc 0.15` residual is documented in the ticket's `### Known
Limitations` citing ticket 132, per the plan's framing.

No earlier artifact carries a claim invalidated by this run: the RCA's
root cause, the reproduction tests, and the plan's fix designs all held;
the one plan claim corrected during implementation (W4's TouAware scope)
is recorded above and in the ticket's Deviations, and the composer test
`targetless_rate_winner_keeps_ceiling_target` pins the corrected behaviour.



## Done-challenge produced changes — 2026-09-14T21:20:17.654Z

- **Phase:** fix
- **Run:** I-05-fix-6bc681f7-4979-4e53-94db-76964c0c5c7d

## Done-challenge produced changes — 2026-09-14T21:28:42.492Z

- **Phase:** round-out-coverage
- **Run:** I-05-round-out-coverage-48bfc0b7-8c2d-4f96-b469-2d9ad570258e

## Done-challenge produced changes — 2026-09-14T21:45:12.650Z

- **Phase:** red-team
- **Run:** I-05-red-team-7703fc71-6a4c-4065-8887-49d596bcaf64

## Done-challenge produced changes — 2026-09-14T21:56:51.812Z

- **Phase:** fix
- **Run:** I-05-fix-0f6f8669-0263-4b4b-9995-ad4459c662db

## Done-challenge produced changes — 2026-09-14T22:11:40.089Z

- **Phase:** red-team
- **Run:** I-05-red-team-ecab3bf9-a376-4bc8-9b4d-8b1e4f8b7180

## Operator-directed driver-response model — 2026-09-14 (fix run v2-fix-b56a3759)

The operator rejected a ticket-only disposition for the human driver's
range-shortfall responses ("no you lazy fuck", "HARES must be a high
quality, robust simulator for capturing and reporting this behavior",
"you do what is right") and directed the model be implemented. What
landed, mapped to the operator's stated decision tree:

1. **Minimal home charge** ("a charge at home may do a minimal charge to
   get home with a safe reserve pack amount"): the range-anxiety override
   now dispatches `SOCTarget{anxiety band}` — trip + safe reserve —
   instead of `SOCTarget{1.0}`. The urgency gate is unchanged (a driver
   below the band with plenty of time still lets their strategy govern).
2. **Wouldn't drive** ("if a human driver was short, they wouldn't
   drive"): at the departure minute, a trip the observed pack cannot
   cover is CANCELLED — the vehicle stays plugged in, the day self-heals
   overnight under the strategy (with the anxiety backstop), and the
   cancellation is counted in the new cumulative `drive_cancelled`
   telemetry channel and warned once. This SUPERSEDES the pass-3
   departure-truncation semantics; the equipment's mid-trip
   truncation accounting (`DRIVE_SHORTFALL_KWH`) remains as the physical
   backstop for mid-trip capacity fade.
3. **Charge out, skip the home cycle** ("they may do a usual charge back
   up to their usual needed amount out, skipping a home charge cycle"):
   a driver with away charging configured who arrives short of their
   usual amount charges out back up to it (the away recoup takes the
   full gap to usual over the habitual fraction, and the away session is
   bounded by an explicit `SOCTarget{usual}`); arriving home at the
   usual target, the evening's home session transfers nothing — the
   skip is emergent and exact.
4. **Reporting**: new `range_anxiety_active` telemetry channel (the
   driver-is-short state, predictive of out-of-pattern charging) and the
   `drive_cancelled` counter, both actor-checkpoint-persistent where
   applicable.

**Red-listed test re-baseline, transparently:** pass-3's
`drive_shortfall_is_accounted_not_silently_dropped` pinned the
truncation semantics ("the vehicle departs and the drive is delivered up
to the pack"). The operator's model changes that contract at the
departure-short case — the vehicle does not depart. The test was
re-baselined to the new contract PRESERVING its protective invariants
(no dispatched drive step the equipment would reject; every dispatched
kWh reaches the pack; the shortfall is reported, never silently
dropped): with cancellation, no `EvDrive` exists to reject, and the
shortfall is counted in `drive_cancelled`. This is a recorded
operator-directed contract change, not a gate quieted — the re-baselined
test still fails if cancellation regresses (drive dispatches would
appear) or if the counter stops being reported.

Prior artifacts carrying the superseded semantics: `red-team-note.md`
(pass 3) describes the truncation behavior as the fix; the ticket's
Implementation Notes pass-3 paragraph now records the supersession. The
`soc_clamps_at_boundaries` equipment-level overdraw section (mid-trip
truncation accounting) is unaffected and still pins the equipment
backstop.

## Done-challenge produced changes — 2026-09-15T00:13:54.655Z

- **Phase:** fix
- **Run:** I-05-fix-1a1a0da0-67fb-46e7-9baf-b75540ba80ff

## Done-challenge produced changes — 2026-09-15T00:27:40.833Z

- **Phase:** red-team
- **Run:** I-05-red-team-4c17e30c-dbac-431d-9301-3d4c082af7a5

## Done-challenge produced changes — 2026-09-15T04:02:56.606Z

- **Phase:** fix
- **Run:** I-05-fix-a88817be-0cca-40a1-93d8-7606e2b818bf

## Done-challenge produced changes — 2026-09-15T04:14:55.954Z

- **Phase:** round-out-coverage
- **Run:** I-05-round-out-coverage-2ca80c50-2fb8-4bdc-b479-1ecda47f3de0

## Done-challenge produced changes — 2026-09-15T07:53:01.382Z

- **Phase:** fix
- **Run:** I-05-fix-8a3bf21d-cfc1-4e98-8605-0d80ed0a3bbe

## Done-challenge produced changes — 2026-09-15T08:05:48.856Z

- **Phase:** round-out-coverage
- **Run:** I-05-round-out-coverage-0f95dfb9-f224-41a8-b29a-6831f472b774

## Done-challenge produced changes — 2026-09-15T08:18:50.937Z

- **Phase:** round-out-coverage
- **Run:** I-05-round-out-coverage-44878c0e-4cd7-4f20-ae9a-5712d88e995e

## Done-challenge produced changes — 2026-09-15T16:25:27.197Z

- **Phase:** red-team
- **Run:** I-05-red-team-acf91923-80f0-4e2b-96ad-03faeb975f0f