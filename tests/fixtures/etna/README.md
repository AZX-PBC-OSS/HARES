# Section 13 empirical-validation data provenance

The measured data and thresholds this directory carries are the public
ASHRAE 140-2023 Addendum b Section 13 files (the ETNA twin-cell
steady-state empirical-validation suite), hash-pinned. The empirical
suite's comparison contract reads only these committed files: the tests
never touch the network.

## Sources

1. The addendum PDF (the case specifications, Table 13-1, the comparison
   windows, the u(Qhtr) percentages):

   https://www.ashrae.org/file%20library/technical%20resources/standards%20and%20guidelines/standards%20addenda/140_2023_b_20260424.pdf

   sha256 `7257eb68b09faa3c9350cb9bc85a8e6c5b5fd89985c7adc07a070b84aca267df`

2. The Section 13 Analysis Files bundle (the public measurement data and
   the acceptance-threshold configuration):

   https://data.ashrae.org/standard140/AnalysisFiles/Section13_Analysis_Files.zip

   sha256 `970927ec622edf237b715f3c9fd2649d0bd4a7a421c5720900b10addf38fa058`

## Files in this directory

| File | Content | sha256 |
| --- | --- | --- |
| `MeasurementData.json` | byte-exact archive member: the hourly measured series for the four datasets (ET110A, ET110B, ET100A, ET100B) with the propagated uncertainty-band columns | `a189ddb514d38ec9ce97dca8d22f9b261a5f621b0d5866e52e85482e7dfa42d0` |
| `ET_config.yaml` | byte-exact archive member: `best_ss_time_periods` (the best steady-state windows) and `AccCrit_Ke_CVRMSE` (the informative per-case CV(RMSE) acceptance thresholds) | `eb02e96bd11e610972c8b0802163bcf83373edebd56a5243efa733ddc0523453` |
| `ExampleSoftwareQhtr.json` | extracted from the archive member `ExampleSoftwareData.json`: the example programs' hourly Qhtr for the six Section 13 cases restricted to each case's best steady-state window | `ae49e0fded88f392d66acfd0c2b64acae940dcc025625faef294f5ca1b0bd6bd` |
| `et_cases.toml` | the six case contracts: identity, comparison windows, measured values with uncertainty bounds, thresholds | recompute with `sha256sum et_cases.toml` after any edit |

Archive members not committed (their content is either byte-exact above
or summarized):

| Archive member | sha256 | Disposition |
| --- | --- | --- |
| `ExampleSoftwareData.json` | `6db35b7331d0719b3fbe5778e29bac1fc687c9560d8be917d9f66fba1f823a4f` | 48 MB full-year hourly series for every sheet; `ExampleSoftwareQhtr.json` extracts the six comparison windows |
| `ET_analysis.py` | `6dc6d8a5913f7520297d04f906823b37e939c8b4c48e730f51f01b3042769586` | ASHRAE's own comparison script; the kernel mirrors its Qhtr comparison |
| `ET_AccCrit_processor.py` | `b29863999c26c260db78c570a300bf6fa8e466fcee5809b2c2d8000dddd399e1` | ASHRAE's own acceptance-criteria script; the kernel mirrors its CV(RMSE) formula and B12.2 rounding tolerance |
| `Std140_ET_Results_Tables_template.xlsx` | `fa732663c3bf517ff38a8d380e7ae188162dcb737caacca6708340acf863051b` | results-tables template; not consumed |

The normative `Std140_ET_Output.xlsx` output-report template is
distributed only inside the purchasable standard media
(`140-2023-AccompanyingFiles.zip`, "available only to purchasers"); it
is not publicly pinnable and is not committed. The template's hourly
column layout for each case sheet is published in the addendum's
Tables 13-50 through 13-55; the empirical harness's output-template
support owns that layout in its own comparison code.

## The `MeasurementData.json` NaN normalization

The archive's `MeasurementData.json` carries Python's non-standard `NaN`
literal in the summary columns (`QhtrA_avg`/`QhtrB_avg`,
`Qhtr_lower_avg`, `Qhtr_upper_avg`) for every hour outside the best
steady-state window (3291 occurrences: 364x3 in ET110A, 364x3 in ET110B,
184x3 in ET100A, 185x3 in ET100B). The committed file is byte-exact
archive content; the loader replaces the `NaN` token with `null` in
memory before parsing (no string value in the file contains the
substring, so the token replacement is unambiguous), and the loader's
integrity test asserts the per-group null counts match the numbers above.

## Extraction procedure for `ExampleSoftwareQhtr.json`

1. Download and verify the bundle's sha256.
2. Unzip; read `ExampleSoftwareData.json` (pandas-style columns/index/data).
3. For each of the six cases in `et_cases.toml`, for each program that
   carries the case's sheet: take the hourly `Qhtr` values between the
   case's best steady-state window bounds from the same JSON.
4. Assert each extracted series is finite and matches the window's hour
   count; record the program versions from the addendum's Informative
   Annex B8.4.4 nomenclature.
5. Write with `json.dumps(indent=1, sort_keys=True)` and record the
   output's sha256 in the loader's hash pins (`tests/etna/measured.rs`).

## Units

- All temperatures: degrees Celsius.
- `Qhtr`, `Qfan`: watt-hours per hour (the hour-integrated average power
  over the preceding hour).
- `HWDsafr`: cubic metres per hour.
- Uncertainty: `qhtr_uncertainty_rel` and `qhtr_acccrit_cvrmse_max` are
  dimensionless fractions of the measured average.
