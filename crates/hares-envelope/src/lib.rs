//! RC network envelope model with state-space solvers.

pub mod boundary_rc;
pub mod electrical_solver;
pub mod fluid_solver;
pub mod humidity_solver;
pub mod longwave_radiation;
pub mod rc_network;
pub mod state_space;
pub mod thermal_solver;

pub use boundary_rc::{
    BoundaryDiagnostic, BoundaryInput, BoundaryRcError, BuildingRC, EnvelopeDiagnostics,
    ExteriorTarget, InteriorLwrMethod, LayerInput, PrecomputedRCLayer, RCPath, SurfaceLayerInfo,
    ZoneInput, assemble_building_rc, derive_zone_capacitances, parallel_path_conductivity,
};
pub use electrical_solver::{
    ElectricalSolver, ElectricalSolverConfig, ElectricalSolverError, SolverZipCoefficients,
};
pub use fluid_solver::{FluidSolver, FluidSolverConfig};
pub use humidity_solver::{HumiditySolver, HumiditySolverConfig};
pub use longwave_radiation::{
    EMISSIVITY_DEFAULT, EMISSIVITY_RADIANT_BARRIER, EMISSIVITY_WINDOW, ExteriorSurface,
    INTERIOR_SOLAR_ABSORPTANCE_DEFAULT, InteriorSurface, SOLAR_ABSORPTANCE_DEFAULT,
    SOLAR_ABSORPTANCE_RADIANT_BARRIER, STEFAN_BOLTZMANN, exterior_longwave_w,
    exterior_longwave_w_m2, interior_longwave_linearised_w, interior_longwave_net_w,
    linearised_h_r, sky_view_factor,
};
pub use rc_network::{NodeId, RCNetwork, RCNetworkError, parallel_resistance};
#[cfg(feature = "observe_detailed")]
pub use state_space::gershgorin_false_positive_count;
pub use state_space::{
    CouplingData, OutputMapping, SolveTarget, SolverScratch, StabilityResult, StateSpaceError,
    StateSpaceModel, ZERO_GAIN_EPSILON, discretize_auto, discretize_zoh, eigenvalue_check,
    matrix_exp, van_loan_discretize,
};
pub use thermal_solver::{
    BoundaryCategory, BoundaryDiagnosticInfo, DrivingTemp, EnvelopeComponentGains,
    ExteriorSurfaceInfo, FilmCoefficientModel, InfiltrationMethod, InteriorConvectionInjection,
    InteriorLwrZoneConfig, InteriorSolarSurfaceInfo, InteriorSolarZoneConfig, InteriorSurfaceInfo,
    MechanicalVentilationParams, NaturalVentilationConfig, StateSpaceWiring, ThermalSnapshot,
    ThermalSolver, ThermalSolverConfig, ThermalSolverError, WindowSolarProperties,
    ZoneSensibleBreakdown,
};
