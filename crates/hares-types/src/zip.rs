//! Shared ZIP load model: voltage-dependent real/reactive power scaling.
//!
//! Single home of the ZIP polynomial and the power-factor → reactive-power
//! conversion used across HARES equipment (scheduled loads, event-based
//! loads, water heaters, HVAC, PV). Ported from OCHRE `Equipment.py`
//! `run_zip` (lines 200-218):
//!
//! - `P_actual = P * (zp * V² + ip * V + pp)` where `V = v / v0` (per-unit)
//! - `Q_actual = P_actual * tan(acos(pf)) * (zq * V² + iq * V + pq)`
//!
//! Sign convention: reactive power is signed, positive = inductive/lagging
//! (absorbing vars), matching `CoreFlows::reactive_power_var`.
//!
//! Divergence from OCHRE (intentional, kept): OCHRE cross-wires the real and
//! reactive coefficient arrays at off-nominal voltage (`Equipment.py:211-214`);
//! HARES applies the real coefficients to P and the reactive coefficients to
//! Q, which is the physically correct wiring.

use serde::{Deserialize, Serialize};

/// ZIP load model coefficients plus power factor.
///
/// Field names (`zp`/`ip`/`pp`/`zq`/`iq`/`pq`/`pf`) match the entries in
/// `defaults/zip_parameters.toml`, so the struct deserializes directly from
/// that file's rows. `v0` is optional in serialized form and defaults to 1.0.
///
/// `pf = 0.0` is the sentinel for "no reactive ZIP configured" — it produces
/// zero reactive power (see [`ZipLoad::tan_phi`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ZipLoad {
    /// Real-power impedance fraction (Z term).
    pub zp: f64,
    /// Real-power current fraction (I term).
    pub ip: f64,
    /// Real-power constant-power fraction (P term).
    pub pp: f64,
    /// Reactive-power impedance fraction.
    pub zq: f64,
    /// Reactive-power current fraction.
    pub iq: f64,
    /// Reactive-power constant fraction.
    pub pq: f64,
    /// Power factor magnitude for reactive power calculation.
    /// `0.0` is the sentinel for "no reactive ZIP configured": a
    /// reactive-only switch that zeroes the reactive power of the whole
    /// unit (primary and secondary components) and never alters real
    /// power: the real coefficients govern as resolved, and a secondary
    /// fan/pump's real response evaluates its own physical component row
    /// (see `hares_equipment`'s `hvac::reactive`).
    pub pf: f64,
    /// Reference voltage for ZIP normalization [per-unit]. Default 1.0.
    #[serde(default = "default_v0")]
    pub v0: f64,
}

fn default_v0() -> f64 {
    1.0
}

/// A [`ZipLoad`] plus the regime it was resolved under: whether the
/// real-power coefficients govern the equipment's real power, or are the
/// DER controller's reactive-only view with real power coming from
/// elsewhere.
///
/// A bare `ZipLoad` cannot express this distinction: a typed physics
/// model (HVAC, water heaters, ventilation) resolves its class row and
/// the row's real coefficients scale the physics-computed draw at the bus
/// voltage (the declared deep-sag response), while a scheduled load
/// scales its *scheduled* draw through the same coefficients, and a DER
/// controller (battery, PV, EV) computes real power entirely on its own:
/// its resolved row is reactive-only and its real draw is never
/// ZIP-scaled. The coefficients alone are therefore ambiguous; the regime
/// must travel with them.
///
/// Derefs to [`ZipLoad`] for field access and computation (`apply`,
/// `reactive_var`), so equipment that stores a `ResolvedZip` reads exactly
/// like one that stores a `ZipLoad`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedZip {
    /// The ZIP coefficients.
    pub zip: ZipLoad,
    /// Whether `zip`'s real-power coefficients govern this equipment's real
    /// power at the bus voltage. `false` means the real side is the DER
    /// reactive-only view: real power comes from the equipment's own
    /// controller and only the reactive side of `zip` applies.
    pub real_power_zip_applies: bool,
    /// The deep-sag shape policy for the real polynomial: `true` evaluates
    /// the row as fitted (the recorded motor families whose deep-sag
    /// elevation is the measured static draw the record licenses),
    /// `false` evaluates through the monotone-in-voltage form
    /// ([`ZipLoad::deep_sag_real_multiplier`]).
    pub real_shape_as_fitted: bool,
}

impl ResolvedZip {
    /// A ZIP whose real-power coefficients govern the equipment's real
    /// power (scheduled and event loads: the scheduled draw is scaled by
    /// the real-power polynomial at the bus voltage each step; typed
    /// physics equipment: the physics-computed draw is scaled the same
    /// way). The deep-sag shape defaults to the monotone form; the
    /// resolution chain sets the recorded policy for the class.
    #[must_use]
    pub fn governing(zip: ZipLoad) -> Self {
        Self {
            zip,
            real_power_zip_applies: true,
            real_shape_as_fitted: false,
        }
    }

    /// The DER reactive-only regime: real power is computed by the
    /// equipment's own controller (battery BMS, PV inverter, EV charging
    /// strategy) and stays voltage-invariant, so only the reactive side of
    /// `zip` is used and the real side is pinned to constant power
    /// `(0, 0, 1)` -- the same pin [`ZipLoad::reactive_only`] applies.
    #[must_use]
    pub fn reactive_only(zip: ZipLoad) -> Self {
        Self {
            zip: ZipLoad {
                zp: 0.0,
                ip: 0.0,
                pp: 1.0,
                ..zip
            },
            real_power_zip_applies: false,
            real_shape_as_fitted: false,
        }
    }

    /// The equipment's real-power multiplier at the bus voltage: the
    /// resolved row evaluated through its recorded deep-sag shape.
    #[must_use]
    pub fn deep_sag_real_multiplier(&self, voltage_pu: f64) -> f64 {
        self.zip
            .deep_sag_real_multiplier(voltage_pu, self.real_shape_as_fitted)
    }
}

/// Largest plausible ZIP coefficient magnitude. The literature table
/// (`zip_defaults_for_class`) tops out near 28.6 (Refrigerator `iq`);
/// 100 gives an order of magnitude of headroom while staying far below
/// any overflow territory — a row whose products could overflow
/// `f64::MAX` needs coefficients near 1e308, six orders past physics.
pub const ZIP_COEFFICIENT_MAGNITUDE_MAX: f64 = 100.0;

/// Plausible per-unit reference-voltage range. `v0` is the per-unit base
/// the measured voltage is normalized against (`v / v0`); every literature
/// row uses 1.0. [0.1, 10] admits references an order of magnitude off
/// base while keeping the normalized band voltage (±5 % service band over
/// `1 / v0`) bounded well away from overflow.
pub const ZIP_V0_MIN: f64 = 0.1;
pub const ZIP_V0_MAX: f64 = 10.0;

/// Reject ZIP rows whose magnitudes cannot be physically meaningful,
/// naming the offending field.
///
/// Coefficient sums and point probes cannot bound band behaviour: a row
/// like `zp = 1.79e308, ip = -1.79e308, pp = 1.0` cancels to exactly 1.0
/// at nominal voltage (passing sum checks and any single-voltage probe)
/// yet produces `inf - inf = NaN` real power at 1.05 pu. Magnitude bounds
/// make the overflow class unrepresentable instead of sampling it: with
/// every coefficient within [`ZIP_COEFFICIENT_MAGNITUDE_MAX`] and `v0`
/// within [`ZIP_V0_MIN`, `ZIP_V0_MAX`], the largest product across the ±5 %
/// service band is ~1.2e21 (including the `tan(acos(pf))` bound just
/// above the `1e-9` sentinel), far below `f64::MAX`.
///
/// `pf` is cos(phi) and lies in [-1, 1]: 0.0 stays the "no reactive"
/// sentinel and negative values the capacitive sign convention. A finite
/// pf outside the range passes every numeric guard yet clamps silently at
/// every use (`tan(acos(clamp(pf)))` zeroes or sign-flips Q) while being
/// published verbatim — a wrong config accepted without a signal.
pub fn validate_plausible_magnitudes(zip: &ZipLoad) -> Result<(), String> {
    let coefficients = [
        ("zp", zip.zp),
        ("ip", zip.ip),
        ("pp", zip.pp),
        ("zq", zip.zq),
        ("iq", zip.iq),
        ("pq", zip.pq),
    ];
    for (field, value) in coefficients {
        if value.abs() > ZIP_COEFFICIENT_MAGNITUDE_MAX {
            return Err(format!(
                "ZIP coefficient {field} = {value} exceeds the plausible \
                 magnitude {ZIP_COEFFICIENT_MAGNITUDE_MAX} (literature rows \
                 top out near 28.6)"
            ));
        }
    }
    if zip.pf.abs() > 1.0 {
        return Err(format!(
            "ZIP power factor pf = {} is outside the physical [-1, 1] range \
             (cos(phi); 0.0 is the no-reactive sentinel, negative values the \
             capacitive sign convention)",
            zip.pf
        ));
    }
    if !(ZIP_V0_MIN..=ZIP_V0_MAX).contains(&zip.v0) {
        return Err(format!(
            "ZIP reference voltage v0 = {} is outside the plausible \
             per-unit range [{ZIP_V0_MIN}, {ZIP_V0_MAX}] (literature rows \
             use 1.0)",
            zip.v0
        ));
    }
    Ok(())
}

impl std::ops::Deref for ResolvedZip {
    type Target = ZipLoad;
    fn deref(&self) -> &ZipLoad {
        &self.zip
    }
}

impl Default for ZipLoad {
    fn default() -> Self {
        Self::constant_power()
    }
}

impl ZipLoad {
    /// Constant-power load with no reactive component: real and reactive
    /// polynomials are `(0, 0, 1)` and `pf = 0.0` (the "no reactive"
    /// sentinel). [`ZipLoad::apply`] leaves real power untouched and
    /// produces zero reactive power. Equivalent to `Default`.
    pub fn constant_power() -> Self {
        Self {
            zp: 0.0,
            ip: 0.0,
            pp: 1.0,
            zq: 0.0,
            iq: 0.0,
            pq: 1.0,
            pf: 0.0,
            v0: 1.0,
        }
    }

    /// Reactive-only ZIP: the real side is forced to constant power
    /// `(0, 0, 1)` so [`ZipLoad::apply`] never modifies real power.
    ///
    /// This is the Rule R1 constructor for typed equipment: reactive power is
    /// computed *from* the already-computed real power via
    /// [`ZipLoad::reactive_var`], never *through* the real-power ZIP
    /// polynomial, guaranteeing bit-identical real power at all voltages.
    pub fn reactive_only(zq: f64, iq: f64, pq: f64, pf: f64) -> Self {
        Self {
            zp: 0.0,
            ip: 0.0,
            pp: 1.0,
            zq,
            iq,
            pq,
            pf,
            v0: 1.0,
        }
    }

    /// Reactive/active power ratio `tan(acos(pf))`.
    ///
    /// OCHRE Equipment.py:74 constructs `pf_mult = np.tan(np.arccos(kwargs["pf"]))`.
    /// Returns `0.0` when `pf ≈ 0` — the sentinel for "no ZIP reactive
    /// coefficients configured" — skipping `tan(acos(0.0))` which diverges.
    pub fn tan_phi(&self) -> f64 {
        if self.pf.abs() < 1e-9 {
            0.0
        } else {
            // The tan and acos come from the libm crate's pure-Rust
            // implementations, not the platform's: the linux capture and the
            // mac leg must evaluate the same bits, and the two platform
            // libms disagree on the last bit for some inputs.
            libm::tan(libm::acos(self.pf.clamp(-1.0, 1.0)))
        }
    }

    /// Apply full ZIP voltage-dependent scaling to a real power (kW).
    ///
    /// Returns `(real_w, reactive_var)`.
    /// Returns `(0.0, 0.0)` when `p_w` is zero or voltage is zero (grid
    /// outage). `pf` is a power factor (cos φ); reactive power converts via
    /// `tan(acos(pf))` to obtain the reactive/active power ratio. When
    /// `pf ≈ 0`, no reactive power is produced — `pf = 0` is the default
    /// sentinel for "no ZIP reactive coefficients configured."
    pub fn apply(&self, p_w: f64, voltage_pu: f64) -> (f64, f64) {
        if p_w == 0.0 || voltage_pu == 0.0 {
            return (0.0, 0.0);
        }
        let v_norm = voltage_pu / self.v0;
        let zip_multiplier = self.zp * v_norm * v_norm + self.ip * v_norm + self.pp;
        let real_w = p_w * zip_multiplier;
        let reactive_base = self.zq * v_norm * v_norm + self.iq * v_norm + self.pq;
        // pf = 0 is the sentinel for "no reactive ZIP configured" — skip
        // tan(acos(0.0)) which diverges, and produce zero reactive power.
        let reactive_var = if self.pf.abs() < 1e-9 {
            0.0
        } else {
            let tan_phi = libm::tan(libm::acos(self.pf.clamp(-1.0, 1.0)));
            real_w * tan_phi * reactive_base
        };
        (real_w, reactive_var)
    }

    /// Reactive power (kVAR) from an already-computed real power (kW)
    /// (Rule R1: Q-only, real power is never touched).
    ///
    /// `Q = p_w * tan(acos(pf)) * (zq * V² + iq * V + pq)` with
    /// `V = voltage_pu / v0`. Zero when `pf` is the `0.0` sentinel.
    /// Returns `0.0` when `p_w` is zero or voltage is zero (grid outage),
    /// matching the guard in [`ZipLoad::apply`] — de-energized equipment
    /// draws no vars.
    pub fn reactive_var(&self, p_w: f64, voltage_pu: f64) -> f64 {
        if p_w == 0.0 || voltage_pu == 0.0 {
            return 0.0;
        }
        let v_norm = voltage_pu / self.v0;
        let reactive_base = self.zq * v_norm * v_norm + self.iq * v_norm + self.pq;
        p_w * self.tan_phi() * reactive_base
    }

    /// The real-power multiplier of the deep-sag form: the row's real
    /// polynomial evaluated at the ACTUAL per-unit voltage (`V =
    /// voltage_pu / v0`), exactly the identity at the reference voltage.
    ///
    /// Shape constraint (recorded in `docs/PHYSICS_DECISIONS.md`, the
    /// deep-sag entry): the evaluated response is monotone non-decreasing
    /// in voltage across the admitted [0.5, 1.5] pu band for every load
    /// class whose physics is monotone:
    ///
    /// - the resistive classes: monotone increasing through their
    ///   impedance share (P = V²/R);
    /// - the constant-power share: flat by declaration;
    /// - the recorded motor families (the Hajagos & Danai 1998 pumps and
    ///   compressor rows, and the fan family): the row as fitted, for
    ///   their deep-sag elevation is the measured static draw the record
    ///   licenses, so `as_fitted = true` evaluates the raw polynomial;
    /// - every other class (`as_fitted = false`): a row whose stationary
    ///   point falls inside the admitted band below nominal is held at the
    ///   stationary value on the deeper-sag side, so the evaluated
    ///   response never rises as the voltage falls. The stalled-draw class
    ///   the record does not model is thereby unreachable: no evaluated
    ///   multiplier exceeds its nominal value at reduced voltage.
    ///
    /// Rows with no in-band stationary point evaluate raw (they are
    /// already monotone on the band), the identity at the reference
    /// voltage is exact (the validated coefficient sums make the model a
    /// pure redistribution there; the polynomial's last-ulp residue at
    /// `v == v0` is arithmetic, not physics: this is what keeps the
    /// nominal-voltage evaluation exact), and no value is ever clamped to
    /// a band edge: the form evaluates at any finite voltage a caller can
    /// construct.
    #[must_use]
    pub fn deep_sag_real_multiplier(&self, voltage_pu: f64, as_fitted: bool) -> f64 {
        let v_norm = voltage_pu / self.v0;
        if v_norm == 1.0 {
            return 1.0;
        }
        // The monotone hold lives on the sub-nominal side, where the
        // stalled-draw class lives. A minimum (zp > 0) whose stationary
        // point falls inside the admitted band below nominal would
        // otherwise rise as the voltage falls past it.
        if !as_fitted && v_norm < 1.0 && self.zp > 0.0 {
            let v_star = -self.ip / (2.0 * self.zp);
            if v_star >= 0.5 / self.v0 && v_star < 1.0 {
                let held = if v_norm < v_star { v_star } else { v_norm };
                return self.zp * held * held + self.ip * held + self.pp;
            }
        }
        self.zp * v_norm * v_norm + self.ip * v_norm + self.pp
    }
}

/// Every class name recognized by [`zip_defaults_for_class`], including the
/// HARES extensions. Kept in sync with the `match` arms below; the
/// completeness test in this module and the toml drift test in hares-io both
/// iterate this list.
pub const ZIP_CLASS_NAMES: &[&str] = &[
    "Lighting",
    "Indoor Lighting",
    "Exterior Lighting",
    "Garage Lighting",
    "Basement Lighting",
    "Refrigerator",
    "Freezer",
    "MELs",
    "Basement MELs",
    "Plug Loads",
    "TV",
    "Well Pump",
    "Pool Pump",
    "Spa Pump",
    "Pool Heater",
    "Spa Heater",
    "Ceiling Fan",
    "Ventilation Fan",
    "HRV",
    "ERV",
    "Clothes Washer",
    "Clothes Dryer",
    "Dishwasher",
    "Range",
    "Cooking Range",
    "ASHP Heater",
    "MSHP Heater",
    "GSHP Heater",
    "WSHP Heater",
    "Electric Baseboard",
    "Electric Furnace",
    "Electric Boiler",
    "Air Conditioner",
    "ASHP Cooler",
    "MSHP Cooler",
    "Room AC",
    "GSHP Cooler",
    "WSHP Cooler",
    "Dehumidifier",
    "Gas Furnace",
    "Gas Boiler",
    "Gas Water Heater",
    "Ideal Cooler",
    "Ideal Heater",
    "Ideal HVAC",
    "Electric Resistance Water Heater",
    "Heat Pump Water Heater",
    "Tankless Water Heater",
    "Gas Tankless Water Heater",
];

/// Whether the class's recorded deep-sag model evaluates its row as
/// fitted: the Hajagos & Danai 1998 motor families (the pumps and the
/// compressor rows) and the fan motor family. The deep-sag decision
/// (`docs/PHYSICS_DECISIONS.md`) licenses their deep-sag elevation as the
/// measured static draw of the family, fitted across the sag band; no
/// other class carries that license, so every other class evaluates
/// through the monotone form ([`ZipLoad::deep_sag_real_multiplier`]):
/// the refrigerator/freezer class's fitted quadratic turns at 0.843 pu
/// inside the admitted band, and its extrapolated 1.4675x draw at 0.5 pu
/// is the stalled-draw class the record does not model.
///
/// Kept in sync with the motor-family arms of [`zip_defaults_for_class`];
/// the completeness test in this module checks that every class named here
/// resolves to one of the H&D or fan rows.
#[must_use]
pub fn deep_sag_row_evaluates_as_fitted(class_name: &str) -> bool {
    matches!(
        class_name,
        "Well Pump"
            | "Pool Pump"
            | "Spa Pump"
            | "Gas Boiler"
            | "ASHP Heater"
            | "MSHP Heater"
            | "GSHP Heater"
            | "WSHP Heater"
            | "Air Conditioner"
            | "ASHP Cooler"
            | "MSHP Cooler"
            | "Room AC"
            | "GSHP Cooler"
            | "WSHP Cooler"
            | "Dehumidifier"
            | "Ceiling Fan"
            | "Ventilation Fan"
            | "HRV"
            | "ERV"
            | "Gas Furnace"
            | "Gas Water Heater"
    )
}

/// Look up literature-based ZIP coefficients from the OCHRE parameter table
/// (`vendors/OCHRE/ochre/defaults/ZIP Parameters.csv`). Returns `None` for
/// equipment classes not found in the table; callers fall back to
/// [`ZipLoad::constant_power`].
///
/// Rows marked "HARES extension" are not in the OCHRE table; each carries a
/// one-line justification. Extensions must be mirrored in
/// `defaults/zip_parameters.toml` (a drift test in hares-io enforces this).
///
/// Coefficients sourced from:
/// - Bokhari et al., IEEE Trans. Power Delivery 29(3), 2014
/// - Hajagos & Danai, IEEE Trans. Power Systems 13(2), 1998
/// - Lu et al., IEEE PESGM, 2008
/// - Arif et al., IEEE Trans. Smart Grid, 2013
///
/// Maintenance invariant: every match arm below MUST also be listed in
/// [`ZIP_CLASS_NAMES`] — the completeness test in this module and the
/// toml drift test in hares-io iterate that list, so an arm missing from
/// it is invisible to both tests (and vice versa: names in the list
/// without an arm fail the completeness test).
pub fn zip_defaults_for_class(class_name: &str) -> Option<ZipLoad> {
    // Lighting (Bokhari et al. 2014)
    const LIGHTING: ZipLoad = ZipLoad {
        zp: 0.54,
        ip: 0.5,
        pp: -0.04,
        v0: 1.0,
        zq: 0.46,
        iq: 0.51,
        pq: 0.03,
        pf: 1.0,
    };
    // Refrigerator (Bokhari et al. 2014)
    const REFRIGERATOR: ZipLoad = ZipLoad {
        zp: 5.03,
        ip: -8.48,
        pp: 4.45,
        v0: 1.0,
        zq: 17.44,
        iq: -28.62,
        pq: 12.18,
        pf: 0.8,
    };
    // MELs — miscellaneous electrical loads
    const MELS: ZipLoad = ZipLoad {
        zp: 0.361_47,
        ip: -0.085_83,
        pp: 0.724_36,
        v0: 1.0,
        zq: 8.399_85,
        iq: -14.167_7,
        pq: 6.767_85,
        pf: 0.8,
    };
    // Pumps (Hajagos & Danai 1998); identical values shared with HVAC_HEAT_PUMP — updating
    // one requires updating the other.
    const PUMPS: ZipLoad = ZipLoad {
        zp: 0.72,
        ip: -0.98,
        pp: 1.26,
        v0: 1.0,
        zq: 14.78,
        iq: -23.71,
        pq: 9.93,
        pf: 0.84,
    };
    // Resistance heater / baseboard (Bokhari et al. 2014)
    const RESISTANCE: ZipLoad = ZipLoad {
        zp: 0.92,
        ip: 0.1,
        pp: -0.02,
        v0: 1.0,
        zq: 0.15,
        iq: 0.86,
        pq: -0.01,
        pf: 1.0,
    };
    // Fan coefficients from OCHRE `fans` row (no primary literature reference;
    // fan ZIP calibration is an open item in the CSV).
    const FAN: ZipLoad = ZipLoad {
        zp: 0.26,
        ip: 0.9,
        pp: -0.16,
        v0: 1.0,
        zq: 0.5,
        iq: 0.62,
        pq: -0.12,
        pf: 0.87,
    };
    // HVAC heat pump / air conditioner (Hajagos & Danai 1998 — same source as PUMPS;
    // values are identical; updating one requires updating the other).
    const HVAC_HEAT_PUMP: ZipLoad = ZipLoad {
        zp: 0.72,
        ip: -0.98,
        pp: 1.26,
        v0: 1.0,
        zq: 14.78,
        iq: -23.71,
        pq: 9.93,
        pf: 0.84,
    };
    const HVAC_COOLING: ZipLoad = ZipLoad {
        zp: 1.6,
        ip: -2.69,
        pp: 2.09,
        v0: 1.0,
        zq: 12.53,
        iq: -21.11,
        pq: 9.58,
        pf: 0.96,
    };
    // Appliances
    const CLOTHES_WASHER: ZipLoad = ZipLoad {
        zp: 0.05,
        ip: 0.31,
        pp: 0.64,
        v0: 1.0,
        zq: -0.56,
        iq: 2.2,
        pq: -0.64,
        pf: 0.65,
    };
    const CLOTHES_DRYER: ZipLoad = ZipLoad {
        zp: 1.0,
        ip: 0.0,
        pp: 0.0,
        v0: 1.0,
        zq: 1.0,
        iq: 0.0,
        pq: 0.0,
        pf: 0.99,
    };
    const DISHWASHER: ZipLoad = ZipLoad {
        zp: 1.0,
        ip: 0.0,
        pp: 0.0,
        v0: 1.0,
        zq: 0.0,
        iq: 0.0,
        pq: 1.0,
        pf: 0.99,
    };
    const RANGE: ZipLoad = ZipLoad {
        zp: 1.0,
        ip: 0.0,
        pp: 0.0,
        v0: 1.0,
        zq: 1.0,
        iq: 0.0,
        pq: 0.0,
        pf: 1.0,
    };
    // Electric Resistance Water Heater — constant impedance (OCHRE CSV row 17)
    const RESISTANCE_WATER_HEATER: ZipLoad = ZipLoad {
        zp: 1.0,
        ip: 0.0,
        pp: 0.0,
        v0: 1.0,
        zq: 1.0,
        iq: 0.0,
        pq: 0.0,
        pf: 1.0,
    };
    // Ideal loads
    const IDEAL: ZipLoad = ZipLoad {
        zp: 0.0,
        ip: 0.0,
        pp: 1.0,
        v0: 1.0,
        zq: 0.0,
        iq: 0.0,
        pq: 1.0,
        pf: 1.0,
    };
    const HPWH: ZipLoad = ZipLoad {
        zp: 0.825,
        ip: -0.44,
        pp: 0.615,
        v0: 1.0,
        zq: 7.465,
        iq: -11.425,
        pq: 4.96,
        pf: 0.97,
    };
    // Tankless water heater electronics (HARES extension, no OCHRE row):
    // switch-mode control electronics / electronic ignition draw at
    // near-unity power factor with no established voltage sensitivity data,
    // so constant power and pf = 1.0 (Q exactly zero, P untouched).
    const TANKLESS: ZipLoad = ZipLoad {
        zp: 0.0,
        ip: 0.0,
        pp: 1.0,
        v0: 1.0,
        zq: 0.0,
        iq: 0.0,
        pq: 1.0,
        pf: 1.0,
    };

    match class_name {
        "Lighting" | "Indoor Lighting" => Some(LIGHTING),
        "Exterior Lighting" => Some(LIGHTING),
        "Garage Lighting" => Some(LIGHTING),
        "Basement Lighting" => Some(LIGHTING),
        "Refrigerator" => Some(REFRIGERATOR),
        "Freezer" => Some(REFRIGERATOR),
        "MELs" | "Basement MELs" => Some(MELS),
        // HARES extension: HPXML "Plug Loads" are the same miscellaneous
        // electrical load population OCHRE labels "MELs".
        "Plug Loads" => Some(MELS),
        // HARES extension: HPXML PlugLoadType="TV other" is split out of the
        // plug-load population as its own "TV" equipment (resolve_loads.rs).
        // Neither the OCHRE ZIP Parameters.csv nor a verifiable Bokhari et
        // al. 2014 row provides TV-specific coefficients, so it inherits the
        // MELs electronics row (pf 0.80) it was split from — without this
        // arm a continuously drawing TV would silently fall back to
        // constant_power() and emit Q ≡ 0.
        "TV" => Some(MELS),
        "Well Pump" | "Pool Pump" | "Spa Pump" => Some(PUMPS),
        "Pool Heater" | "Spa Heater" => Some(RESISTANCE),
        "Ceiling Fan" | "Ventilation Fan" => Some(FAN),
        // HARES extension: HRV and ERV are ventilation fans with electric
        // fan motors (pf 0.87). The ochre_class on ventilation EquipmentConfig
        // may be "HRV" or "ERV" depending on how the dwelling converter
        // constructs the config.
        "HRV" | "ERV" => Some(FAN),
        "Clothes Washer" => Some(CLOTHES_WASHER),
        "Clothes Dryer" => Some(CLOTHES_DRYER),
        "Dishwasher" => Some(DISHWASHER),
        "Range" | "Cooking Range" => Some(RANGE),
        "ASHP Heater" | "MSHP Heater" => Some(HVAC_HEAT_PUMP),
        // HARES extension: ground/water-source heat pump heating is the same
        // compressor motor class as air-source heat pumps (pf 0.84).
        "GSHP Heater" | "WSHP Heater" => Some(HVAC_HEAT_PUMP),
        "Electric Baseboard" | "Electric Furnace" | "Electric Boiler" => Some(RESISTANCE),
        "Air Conditioner" | "ASHP Cooler" | "MSHP Cooler" | "Room AC" => Some(HVAC_COOLING),
        // HARES extension: ground/water-source cooling is the same compressor
        // motor class as air-source cooling equipment (pf 0.96).
        "GSHP Cooler" | "WSHP Cooler" => Some(HVAC_COOLING),
        // HARES extension: dehumidifier compressor behaves like a cooling
        // compressor (mirrors the existing [dehumidifier] toml row).
        "Dehumidifier" => Some(HVAC_COOLING),
        // HARES extension: a gas furnace's electric draw is its blower fan
        // motor (pf 0.87).
        "Gas Furnace" => Some(FAN),
        // HARES extension: a gas boiler's electric draw is its circulation
        // pump motor (pf 0.84).
        "Gas Boiler" => Some(PUMPS),
        // HARES extension: a gas water heater's electric draw is its
        // draft-inducer fan motor (pf 0.87).
        "Gas Water Heater" => Some(FAN),
        "Ideal Cooler" | "Ideal Heater" | "Ideal HVAC" => Some(IDEAL),
        "Electric Resistance Water Heater" => Some(RESISTANCE_WATER_HEATER),
        "Heat Pump Water Heater" => Some(HPWH),
        // HARES extension: tankless water-heater electric draw is control
        // electronics (unity power factor, constant power).
        "Tankless Water Heater" | "Gas Tankless Water Heater" => Some(TANKLESS),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ZIP_CLASS_NAMES as ALL_CLASS_NAMES, ZipLoad, deep_sag_row_evaluates_as_fitted,
        zip_defaults_for_class,
    };

    /// Reference implementation mirroring the legacy
    /// `ZipCoefficients::apply` arithmetic (hares-equipment
    /// scheduled_load.rs) expression-for-expression. `ZipLoad::apply` must
    /// match it bit-for-bit at nonzero power/voltage.
    fn legacy_apply(zip: &ZipLoad, p_w: f64, voltage_pu: f64) -> (f64, f64) {
        let v_norm = voltage_pu / zip.v0;
        let zip_multiplier = zip.zp * v_norm * v_norm + zip.ip * v_norm + zip.pp;
        let real_w = p_w * zip_multiplier;
        let reactive_base = zip.zq * v_norm * v_norm + zip.iq * v_norm + zip.pq;
        let reactive_var = if zip.pf.abs() < 1e-9 {
            0.0
        } else {
            let tan_phi = zip.pf.clamp(-1.0, 1.0).acos().tan();
            real_w * tan_phi * reactive_base
        };
        (real_w, reactive_var)
    }

    #[test]
    fn apply_is_bit_identical_to_legacy_arithmetic_for_all_class_rows() {
        for name in ALL_CLASS_NAMES {
            let zip = zip_defaults_for_class(name)
                .unwrap_or_else(|| panic!("class table missing row for {name:?}"));
            for &v in &[0.9, 1.0, 1.1] {
                for &p_w in &[0.372, 1.0, 4.815] {
                    let (real, reactive) = zip.apply(p_w, v);
                    let (exp_real, exp_reactive) = legacy_apply(&zip, p_w, v);
                    assert_eq!(
                        real.to_bits(),
                        exp_real.to_bits(),
                        "{name}: real power diverged at p={p_w} v={v}: {real} vs {exp_real}"
                    );
                    assert_eq!(
                        reactive.to_bits(),
                        exp_reactive.to_bits(),
                        "{name}: reactive diverged at p={p_w} v={v}: {reactive} vs {exp_reactive}"
                    );
                }
            }
        }
    }

    #[test]
    fn apply_guards_zero_power_and_zero_voltage() {
        let zip = zip_defaults_for_class("ASHP Heater").expect("row");
        assert_eq!(zip.apply(0.0, 1.0), (0.0, 0.0));
        assert_eq!(zip.apply(2.5, 0.0), (0.0, 0.0));
        assert_eq!(zip.apply(0.0, 0.0), (0.0, 0.0));
    }

    /// The real coefficients drive real power and the reactive coefficients
    /// drive reactive power at off-nominal voltage, with distinct arrays on
    /// each side so the wiring is discriminated (DIVERGENCES D-022): the
    /// cross-wired form (real coefficients scaling Q, reactive scaling P)
    /// produces different numbers here.
    #[test]
    fn real_power_follows_the_real_coefficients_and_reactive_the_reactive_off_nominal() {
        let zip = ZipLoad {
            zp: 0.5,
            ip: 0.0,
            pp: 0.5,
            zq: 0.0,
            iq: 1.0,
            pq: 0.0,
            pf: 0.9,
            v0: 1.0,
        };
        let p_w = 2.0;
        let v = 0.5;
        let (real, reactive) = zip.apply(p_w, v);

        // P through (zp, ip, pp): 0.5·0.25 + 0.5 = 0.625 of the draw.
        let expected_real = p_w * (0.5 * v * v + 0.5);
        // Q through (zq, iq, pq) and tan(acos(pf)): 1.0·v of the real base.
        let expected_reactive = expected_real * zip.pf.clamp(-1.0, 1.0).acos().tan() * v;
        assert!(
            (real - expected_real).abs() < 1e-12,
            "{real} vs {expected_real}"
        );
        assert!(
            (reactive - expected_reactive).abs() < 1e-12,
            "{reactive} vs {expected_reactive}"
        );

        // The cross-wired numbers differ: P through (zq, iq, pq) would be
        // p_w·v, Q through (zp, ip, pp) would be p_w·v²·tan(acos(pf))·0.625.
        let cross_real = p_w * v;
        let cross_reactive = p_w * (v * v) * zip.pf.clamp(-1.0, 1.0).acos().tan() * 0.625;
        assert!((real - cross_real).abs() > 1e-9);
        assert!((reactive - cross_reactive).abs() > 1e-9);
    }

    #[test]
    fn reactive_kvar_equals_p_times_tan_acos_pf_at_nominal() {
        // With a unity reactive base (0, 0, 1) at nominal voltage the Q-only
        // path reduces exactly to P·tan(acos(pf)).
        let pf = 0.84;
        let zip = ZipLoad::reactive_only(0.0, 0.0, 1.0, pf);
        let p_w = 3.6;
        let expected = p_w * pf.acos().tan();
        assert_eq!(zip.reactive_var(p_w, 1.0), expected);

        // Class rows have reactive coefficients summing to ~1, so at nominal
        // voltage Q ≈ P·tan(acos(pf)) within floating-point roundoff.
        for name in ALL_CLASS_NAMES {
            let zip = zip_defaults_for_class(name).expect("row");
            if zip.pf.abs() < 1e-9 || (zip.pf - 1.0).abs() < 1e-9 {
                continue;
            }
            let q = zip.reactive_var(p_w, 1.0);
            let expected = p_w * zip.pf.clamp(-1.0, 1.0).acos().tan();
            assert!(
                (q - expected).abs() < 1e-9,
                "{name}: Q at nominal = {q}, expected ≈ {expected}"
            );
        }
    }

    #[test]
    fn pf_zero_sentinel_produces_zero_reactive() {
        let zip = ZipLoad {
            pf: 0.0,
            ..ZipLoad::reactive_only(12.53, -21.11, 9.58, 0.0)
        };
        assert_eq!(zip.tan_phi(), 0.0);
        assert_eq!(zip.reactive_var(5.0, 1.05), 0.0);
        assert_eq!(zip.apply(5.0, 1.05).1, 0.0);
        // constant_power() carries the sentinel by construction.
        assert_eq!(ZipLoad::constant_power().reactive_var(5.0, 1.0), 0.0);
    }

    #[test]
    fn pf_unity_produces_exactly_zero_reactive() {
        // acos(1.0) == 0.0 and tan(0.0) == 0.0 exactly, so Q is exactly zero
        // even with nonzero reactive coefficients (e.g. the Lighting row).
        let zip = zip_defaults_for_class("Lighting").expect("row");
        assert_eq!(zip.pf, 1.0);
        assert_eq!(zip.tan_phi(), 0.0);
        assert_eq!(zip.reactive_var(2.0, 1.1), 0.0);
        assert_eq!(zip.apply(2.0, 1.1).1, 0.0);
    }

    #[test]
    fn reactive_only_leaves_real_power_untouched_at_all_voltages() {
        let zip = ZipLoad::reactive_only(14.78, -23.71, 9.93, 0.84);
        assert_eq!((zip.zp, zip.ip, zip.pp), (0.0, 0.0, 1.0));
        for &v in &[0.9, 1.0, 1.1] {
            let p_w = 2.75;
            let (real, reactive) = zip.apply(p_w, v);
            assert_eq!(real.to_bits(), p_w.to_bits(), "real changed at v={v}");
            assert_eq!(reactive, zip.reactive_var(p_w, v));
        }
    }

    #[test]
    fn constant_power_is_default_and_a_no_op() {
        assert_eq!(ZipLoad::constant_power(), ZipLoad::default());
        let zip = ZipLoad::constant_power();
        for &v in &[0.9, 1.0, 1.1] {
            let (real, reactive) = zip.apply(1.5, v);
            assert_eq!(real.to_bits(), 1.5_f64.to_bits());
            assert_eq!(reactive, 0.0);
        }
    }

    #[test]
    fn governing_keeps_zip_and_declares_real_side_applicable() {
        let zip = zip_defaults_for_class("Lighting").expect("row");
        let resolved = super::ResolvedZip::governing(zip);
        assert_eq!(resolved.zip, zip);
        assert!(resolved.real_power_zip_applies);
        // Deref: field and method access pass through to the coefficients.
        assert_eq!(resolved.pf, zip.pf);
        assert_eq!(resolved.apply(2.0, 1.0), zip.apply(2.0, 1.0));
    }

    #[test]
    fn reactive_only_pins_real_side_and_declares_it_inapplicable() {
        // Whatever the source ZIP says about the real side, the Rule R1
        // constructor pins it to constant power.
        let source = zip_defaults_for_class("Heat Pump Water Heater").expect("row");
        let resolved = super::ResolvedZip::reactive_only(source);
        assert!(!resolved.real_power_zip_applies);
        assert_eq!((resolved.zp, resolved.ip, resolved.pp), (0.0, 0.0, 1.0));
        // The reactive side survives unchanged.
        assert_eq!(
            (resolved.zq, resolved.iq, resolved.pq),
            (source.zq, source.iq, source.pq)
        );
        assert_eq!(resolved.pf, source.pf);
        // Real power is untouched at every voltage.
        for &v in &[0.9, 1.0, 1.1] {
            assert_eq!(resolved.apply(2.4, v).0.to_bits(), 2.4_f64.to_bits());
        }
    }

    #[test]
    fn magnitude_bounds_reject_overflow_class_rows() {
        use super::validate_plausible_magnitudes as validate;
        // Near-maximum cancellation row: sums and point probes pass (the
        // products cancel to exactly 1.0 at nominal) but `inf - inf` NaNs
        // at 1.05 pu — the class the bounds exist to make unrepresentable.
        let cancellation = ZipLoad {
            zp: 1.79e308,
            ip: -1.79e308,
            pp: 1.0,
            ..ZipLoad::constant_power()
        };
        let err = validate(&cancellation).expect_err("cancellation row must be rejected");
        assert!(err.contains("zp"), "error must name the field, got: {err}");
        // Tiny-v0 window (passes a nominal probe, overflows v_norm^2 at
        // 1.05 pu) is closed by the v0 range.
        for v0 in [1e-320_f64, 7.6e-155, 1e200] {
            let zip = ZipLoad {
                v0,
                ..ZipLoad::constant_power()
            };
            let err = validate(&zip).expect_err("out-of-range v0 must be rejected");
            assert!(err.contains("v0"), "error must name v0, got: {err}");
        }
    }

    #[test]
    fn magnitude_bounds_accept_every_literature_row() {
        use super::validate_plausible_magnitudes as validate;
        for name in ALL_CLASS_NAMES {
            let zip = zip_defaults_for_class(name).expect("row");
            validate(&zip).unwrap_or_else(|err| {
                panic!("literature row {name} must pass the magnitude bounds: {err}")
            });
        }
        // Sane non-default references stay accepted.
        validate(&ZipLoad {
            v0: 0.98,
            ..zip_defaults_for_class("ASHP Heater").expect("row")
        })
        .expect("sane v0 must pass");
    }

    #[test]
    fn regime_flag_is_not_derivable_from_coefficients() {
        // The same (0, 0, 1) real side is a legitimate governing model (a
        // scheduled load of unknown class falls back to constant power) and
        // a Rule R1 pin — the flag, not the coefficients, carries which.
        let governing = super::ResolvedZip::governing(ZipLoad::constant_power());
        let pinned = super::ResolvedZip::reactive_only(ZipLoad::constant_power());
        assert_eq!(governing.zip, pinned.zip);
        assert!(governing.real_power_zip_applies);
        assert!(!pinned.real_power_zip_applies);
    }

    #[test]
    fn serde_round_trip_preserves_all_fields() {
        let zip = ZipLoad {
            zp: 0.72,
            ip: -0.98,
            pp: 1.26,
            zq: 14.78,
            iq: -23.71,
            pq: 9.93,
            pf: 0.84,
            v0: 0.98,
        };
        let json = serde_json::to_string(&zip).expect("serialize");
        let back: ZipLoad = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, zip);
    }

    #[test]
    fn serde_v0_defaults_to_one_when_absent() {
        // Field names match defaults/zip_parameters.toml rows, which carry
        // no v0 key.
        let json = r#"{
            "zp": 1.6, "ip": -2.69, "pp": 2.09,
            "zq": 12.53, "iq": -21.11, "pq": 9.58,
            "pf": 0.96
        }"#;
        let zip: ZipLoad = serde_json::from_str(json).expect("deserialize");
        assert_eq!(zip.v0, 1.0);
        assert_eq!(zip, zip_defaults_for_class("Dehumidifier").expect("row"));
    }

    #[test]
    fn class_table_covers_every_known_name() {
        for name in ALL_CLASS_NAMES {
            assert!(
                zip_defaults_for_class(name).is_some(),
                "class table missing row for {name:?}"
            );
        }
        assert_eq!(zip_defaults_for_class("EV"), None);
        assert_eq!(zip_defaults_for_class(""), None);
    }

    #[test]
    fn reactive_kvar_guards_zero_power_and_zero_voltage() {
        // Matches the guard in apply(): de-energized equipment draws no
        // vars, even though the reactive polynomial at v=0 reduces to a
        // nonzero pq term.
        let zip = zip_defaults_for_class("ASHP Heater").expect("row");
        assert_eq!(zip.reactive_var(2.5, 0.0), 0.0);
        assert_eq!(zip.reactive_var(0.0, 1.0), 0.0);
        assert_eq!(zip.reactive_var(0.0, 0.0), 0.0);
        // apply() and reactive_var() agree at v=0.
        assert_eq!(zip.apply(2.5, 0.0).1, zip.reactive_var(2.5, 0.0));
    }

    #[test]
    fn hares_extension_rows_map_to_expected_coefficient_sets() {
        // Plug Loads → MELs values.
        assert_eq!(
            zip_defaults_for_class("Plug Loads"),
            zip_defaults_for_class("MELs")
        );
        // TV (HPXML PlugLoadType="TV other", split out of plug loads) →
        // MELs electronics row; a TV must never fall to constant_power
        // (Q ≡ 0 despite continuous draw).
        assert_eq!(zip_defaults_for_class("TV"), zip_defaults_for_class("MELs"));
        assert_eq!(zip_defaults_for_class("TV").expect("row").pf, 0.8);
        // Dehumidifier → HVAC cooling values.
        assert_eq!(
            zip_defaults_for_class("Dehumidifier"),
            zip_defaults_for_class("Air Conditioner")
        );
        // Ground/water-source heat pumps share air-source rows.
        for name in ["GSHP Heater", "WSHP Heater"] {
            assert_eq!(
                zip_defaults_for_class(name),
                zip_defaults_for_class("ASHP Heater")
            );
            assert_eq!(zip_defaults_for_class(name).expect("row").pf, 0.84);
        }
        for name in ["GSHP Cooler", "WSHP Cooler"] {
            assert_eq!(
                zip_defaults_for_class(name),
                zip_defaults_for_class("ASHP Cooler")
            );
            assert_eq!(zip_defaults_for_class(name).expect("row").pf, 0.96);
        }
        // Gas equipment electric draws: blower fan, circulation pump,
        // draft-inducer fan.
        assert_eq!(
            zip_defaults_for_class("Gas Furnace"),
            zip_defaults_for_class("Ventilation Fan")
        );
        assert_eq!(zip_defaults_for_class("Gas Furnace").expect("row").pf, 0.87);
        assert_eq!(
            zip_defaults_for_class("Gas Boiler"),
            zip_defaults_for_class("Well Pump")
        );
        assert_eq!(zip_defaults_for_class("Gas Boiler").expect("row").pf, 0.84);
        assert_eq!(
            zip_defaults_for_class("Gas Water Heater"),
            zip_defaults_for_class("Ventilation Fan")
        );
        assert_eq!(
            zip_defaults_for_class("Gas Water Heater").expect("row").pf,
            0.87
        );
        // Tankless water heaters: control electronics at unity power factor,
        // constant power (Q exactly zero, P untouched).
        for name in ["Tankless Water Heater", "Gas Tankless Water Heater"] {
            let zip = zip_defaults_for_class(name).expect("row");
            assert_eq!(zip.pf, 1.0);
            assert_eq!((zip.zp, zip.ip, zip.pp), (0.0, 0.0, 1.0));
            assert_eq!(zip.reactive_var(3.2, 1.05), 0.0);
            assert_eq!(zip.apply(3.2, 0.95).0.to_bits(), 3.2_f64.to_bits());
        }
    }

    /// The deep-sag form is exactly the identity at the reference voltage
    /// for every class row, either shape: the validated coefficient sums
    /// make the model a pure redistribution there, and the nominal-voltage
    /// goldens (fixtures at 1.0 pu) depend on the identity being exact.
    #[test]
    fn deep_sag_multiplier_is_exactly_one_at_the_reference_voltage() {
        for name in ALL_CLASS_NAMES {
            let zip = zip_defaults_for_class(name).expect("row");
            for &as_fitted in &[false, true] {
                let m = zip.deep_sag_real_multiplier(1.0, as_fitted);
                assert_eq!(
                    m.to_bits(),
                    1.0_f64.to_bits(),
                    "{name}: the deep-sag multiplier at the reference voltage \
                     must be exactly 1.0 (as_fitted={as_fitted}), got {m}"
                );
            }
        }
    }

    /// F3's shape constraint, the non-motor classes: every class row whose
    /// recorded model is not "as fitted" evaluates monotone non-decreasing
    /// in voltage across the admitted band. The brownout round's witness:
    /// the refrigerator's fitted quadratic turned at 0.843 pu (1.4675x
    /// nominal at 0.5 pu, 0.9787x at 0.7 pu: the draw rose as the voltage
    /// fell, the stalled-draw class the record does not model). The
    /// corrected form holds the row at its stationary point below it, so
    /// the 0.5 pu and 0.7 pu evaluations agree bitwise and no evaluated
    /// multiplier exceeds its nominal value at reduced voltage.
    #[test]
    fn deep_sag_multiplier_is_monotone_across_the_admitted_band_for_non_motor_rows() {
        for name in ALL_CLASS_NAMES {
            if deep_sag_row_evaluates_as_fitted(name) {
                continue;
            }
            let zip = zip_defaults_for_class(name).expect("row");
            let mut previous = f64::NEG_INFINITY;
            for &v in &[0.5, 0.6, 0.7, 0.8, 0.85, 0.9, 0.95, 1.0, 1.1, 1.3, 1.5] {
                let m = zip.deep_sag_real_multiplier(v, false);
                assert!(
                    m >= previous,
                    "{name}: the deep-sag multiplier must be monotone \
                     non-decreasing in voltage, turned at v={v} ({m} after {previous})"
                );
                if v < 1.0 {
                    assert!(
                        m <= 1.0,
                        "{name}: no non-motor load draws above nominal at \
                         reduced voltage without a stall model: m(0.5)-class \
                         value {m} at v={v}"
                    );
                }
                previous = m;
            }
        }
    }

    /// The refrigerator row's corrected evaluation: the held stationary
    /// value below the 0.843 pu turn, bitwise equal at 0.5 and 0.7 pu, and
    /// strictly below the raw polynomial's stalled-draw value at 0.5 pu.
    #[test]
    fn refrigerator_row_holds_at_its_stationary_point_below_the_turn() {
        let zip = zip_defaults_for_class("Refrigerator").expect("row");
        let v_star = -zip.ip / (2.0 * zip.zp);
        assert!(
            (0.5..1.0).contains(&v_star),
            "fixture: the turn at {v_star}"
        );
        let held = zip.deep_sag_real_multiplier(v_star - 0.01, false);
        let m_05 = zip.deep_sag_real_multiplier(0.5, false);
        let m_07 = zip.deep_sag_real_multiplier(0.7, false);
        assert_eq!(m_05.to_bits(), held.to_bits(), "0.5 pu is below the turn");
        assert_eq!(m_07.to_bits(), held.to_bits(), "0.7 pu is below the turn");
        let raw_05 = zip.zp * 0.5 * 0.5 + zip.ip * 0.5 + zip.pp;
        assert!(
            raw_05 > 1.0 && m_05 < 1.0,
            "the raw row extrapolates to the stalled-draw class ({raw_05}) \
             and the corrected form must not ({m_05})"
        );
        // Above the turn the raw polynomial is recovered.
        let m_09 = zip.deep_sag_real_multiplier(0.9, false);
        let raw_09 = zip.zp * 0.9 * 0.9 + zip.ip * 0.9 + zip.pp;
        assert_eq!(m_09.to_bits(), raw_09.to_bits());
    }

    /// F2's recorded motor families: the Hajagos & Danai rows and the fan
    /// row evaluate as fitted: the deep-sag elevation is the measured
    /// static draw the record licenses, so the raw polynomial's values
    /// survive (the air-conditioner row's +14.5 percent at 0.5 pu is the
    /// declared response the brownout round's F2 finding quoted).
    #[test]
    fn motor_family_rows_evaluate_as_fitted() {
        let ac = zip_defaults_for_class("Air Conditioner").expect("row");
        let m_05 = ac.deep_sag_real_multiplier(0.5, true);
        let raw_05 = ac.zp * 0.5 * 0.5 + ac.ip * 0.5 + ac.pp;
        assert_eq!(m_05.to_bits(), raw_05.to_bits());
        assert!(
            raw_05 > 1.1,
            "the recorded elevation must survive: {raw_05}"
        );
        let pumps = zip_defaults_for_class("Well Pump").expect("row");
        let m_pumps = pumps.deep_sag_real_multiplier(0.5, true);
        let raw_pumps = pumps.zp * 0.5 * 0.5 + pumps.ip * 0.5 + pumps.pp;
        assert_eq!(m_pumps.to_bits(), raw_pumps.to_bits());
    }

    /// The policy's roster: every class the deep-sag record names as a
    /// motor family evaluates as fitted; every other class (the
    /// refrigerator among them) evaluates through the monotone form.
    #[test]
    fn deep_sag_policy_rosters_the_recorded_motor_families() {
        const AS_FITTED: &[&str] = &[
            "Well Pump",
            "Pool Pump",
            "Spa Pump",
            "Gas Boiler",
            "ASHP Heater",
            "MSHP Heater",
            "GSHP Heater",
            "WSHP Heater",
            "Air Conditioner",
            "ASHP Cooler",
            "MSHP Cooler",
            "Room AC",
            "GSHP Cooler",
            "WSHP Cooler",
            "Dehumidifier",
            "Ceiling Fan",
            "Ventilation Fan",
            "HRV",
            "ERV",
            "Gas Furnace",
            "Gas Water Heater",
        ];
        for name in AS_FITTED {
            assert!(
                deep_sag_row_evaluates_as_fitted(name),
                "{name} must evaluate as fitted"
            );
        }
        for name in ALL_CLASS_NAMES {
            let expected = AS_FITTED.contains(name);
            assert_eq!(
                deep_sag_row_evaluates_as_fitted(name),
                expected,
                "{name}: the policy roster must match the recorded families"
            );
        }
    }
}
