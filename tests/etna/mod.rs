//! The ASHRAE 140-2023 Addendum b Section 13 empirical-validation test
//! target: the ETNA twin-cell steady-state suite, separate from the
//! annual BESTEST module by construction (this target owns no file with
//! it, and the Section 7 core reference bands carry no Section 13
//! identifier).
//!
//! The suite's structure:
//!
//! - `kernel`: the measured-data comparison kernel, the archive's own
//!   CV(RMSE) acceptance form plus the uncertainty-band comparison,
//!   proved on hand-computed examples and on planted errors before any
//!   case comparison runs.
//! - `measured`: the hash-pinned measured-data loader and the archive's
//!   data-integrity contract.
//! - `registry`: the six-case contract registry and the
//!   example-software cross-checks.
//!
//! The HARES case runs' disposition: the ETNA comparison needs the
//! guard zones' measured hourly temperatures applied as boundary
//! conditions on each test-cell surface (the addendum's recommended
//! Section 13.2.2.7.1 formulation) or as separately controlled zones
//! (Section 13.2.2.7.3), with the case's constant combined surface
//! coefficients settable per surface (the imputed conductivities'
//! derivation assumption). The engine's exterior boundary carries one
//! shared outdoor temperature (`apply_outdoor_inputs`), the films are
//! engine-computed, and the synthetic fixture builder constructs a
//! single conditioned zone, so the case models cannot be expressed
//! through the existing input surfaces. Building that general
//! model surface is a separate operator-approved row; until it lands,
//! the suite pins the measured data, the thresholds, and the comparison
//! kernel, and the example-software comparisons run through the same
//! kernel path the HARES runs will consume.
//!
//! The steady-state convergence contract: each case's comparison window
//! is the standard's own best steady-state determination (the addendum's
//! Sections 13.3.2.4.1 through 13.3.7.4.1, determined per Informative
//! Annex B23.2's temporal uncertainty analysis), the window's hourly
//! counts (18 for the ET110 pair, 55 for the ET100 pair) are pinned in
//! the case contracts, and the in-window measured cell and guard
//! temperatures are pinned at their setpoints. Software results before
//! the window are not part of the comparison (the addendum's own
//! informative note).

mod kernel;
mod measured;
mod registry;
