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
//! (absorbing vars), matching `CoreFlows::reactive_power_kvar`.
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
    /// `0.0` is the sentinel for "no reactive ZIP configured".
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
/// Rule R1 structural pin with real power coming from elsewhere.
///
/// A bare `ZipLoad` cannot express this distinction: a Rule-R1 physics
/// model (HVAC, water heaters, ventilation) publishes `(zp, ip, pp) =
/// (0, 0, 1)` because its real power is computed by its own physics and
/// must stay voltage-invariant, while a scheduled load can legitimately
/// resolve to the *same* `(0, 0, 1)` as a real, governing model (unknown
/// class → [`ZipLoad::constant_power`] fallback, or an explicit sidecar
/// override). The coefficients alone are therefore ambiguous; the regime
/// must travel with them.
///
/// Derefs to [`ZipLoad`] for field access and computation (`apply`,
/// `reactive_kvar`), so equipment that stores a `ResolvedZip` reads exactly
/// like one that stores a `ZipLoad`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedZip {
    /// The ZIP coefficients.
    pub zip: ZipLoad,
    /// Whether `zip`'s real-power coefficients govern this equipment's real
    /// power at the bus voltage. `false` means the real side is the Rule R1
    /// pin: real power comes from the equipment's own physics (or a DER
    /// controller) and only the reactive side of `zip` applies.
    pub real_power_zip_applies: bool,
}

impl ResolvedZip {
    /// A ZIP whose real-power coefficients govern the equipment's real
    /// power (scheduled and event loads: the scheduled draw is scaled by
    /// the real-power polynomial at the bus voltage each step).
    #[must_use]
    pub fn governing(zip: ZipLoad) -> Self {
        Self {
            zip,
            real_power_zip_applies: true,
        }
    }

    /// The Rule R1 regime: real power is computed by the equipment's own
    /// physics (or a DER controller) and must stay voltage-invariant, so
    /// only the reactive side of `zip` is used and the real side is pinned
    /// to constant power `(0, 0, 1)` — the same pin
    /// [`ZipLoad::reactive_only`] applies.
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
        }
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
    /// [`ZipLoad::reactive_kvar`], never *through* the real-power ZIP
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
            self.pf.clamp(-1.0, 1.0).acos().tan()
        }
    }

    /// Apply full ZIP voltage-dependent scaling to a real power [kW].
    ///
    /// Returns `(real_kw, reactive_kvar)`.
    /// Returns `(0.0, 0.0)` when `p_kw` is zero or voltage is zero (grid
    /// outage). `pf` is a power factor (cos φ); reactive power converts via
    /// `tan(acos(pf))` to obtain the reactive/active power ratio. When
    /// `pf ≈ 0`, no reactive power is produced — `pf = 0` is the default
    /// sentinel for "no ZIP reactive coefficients configured."
    pub fn apply(&self, p_kw: f64, voltage_pu: f64) -> (f64, f64) {
        if p_kw == 0.0 || voltage_pu == 0.0 {
            return (0.0, 0.0);
        }
        let v_norm = voltage_pu / self.v0;
        let zip_multiplier = self.zp * v_norm * v_norm + self.ip * v_norm + self.pp;
        let real_kw = p_kw * zip_multiplier;
        let reactive_base = self.zq * v_norm * v_norm + self.iq * v_norm + self.pq;
        // pf = 0 is the sentinel for "no reactive ZIP configured" — skip
        // tan(acos(0.0)) which diverges, and produce zero reactive power.
        let reactive_kvar = if self.pf.abs() < 1e-9 {
            0.0
        } else {
            let tan_phi = self.pf.clamp(-1.0, 1.0).acos().tan();
            real_kw * tan_phi * reactive_base
        };
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        if (self.pf - 1.0).abs() < 1e-9 {
            // `assert!`, not `debug_assert!`: a `debug_assert!` compiles out in
            // release builds even when `check_invariants` is enabled, which
            // would silently void the feature-gated half of the cfg above.
            assert!(
                reactive_kvar.abs() < 1e-9,
                "pf=1.0 should produce zero reactive power, got {} kVAR",
                reactive_kvar
            );
        }
        (real_kw, reactive_kvar)
    }

    /// Reactive power [kVAR] from an already-computed real power [kW]
    /// (Rule R1: Q-only, real power is never touched).
    ///
    /// `Q = p_kw * tan(acos(pf)) * (zq * V² + iq * V + pq)` with
    /// `V = voltage_pu / v0`. Zero when `pf` is the `0.0` sentinel.
    /// Returns `0.0` when `p_kw` is zero or voltage is zero (grid outage),
    /// matching the guard in [`ZipLoad::apply`] — de-energized equipment
    /// draws no vars.
    pub fn reactive_kvar(&self, p_kw: f64, voltage_pu: f64) -> f64 {
        if p_kw == 0.0 || voltage_pu == 0.0 {
            return 0.0;
        }
        let v_norm = voltage_pu / self.v0;
        let reactive_base = self.zq * v_norm * v_norm + self.iq * v_norm + self.pq;
        p_kw * self.tan_phi() * reactive_base
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
    use super::{ZIP_CLASS_NAMES as ALL_CLASS_NAMES, ZipLoad, zip_defaults_for_class};

    /// Reference implementation mirroring the legacy
    /// `ZipCoefficients::apply` arithmetic (hares-equipment
    /// scheduled_load.rs) expression-for-expression. `ZipLoad::apply` must
    /// match it bit-for-bit at nonzero power/voltage.
    fn legacy_apply(zip: &ZipLoad, p_kw: f64, voltage_pu: f64) -> (f64, f64) {
        let v_norm = voltage_pu / zip.v0;
        let zip_multiplier = zip.zp * v_norm * v_norm + zip.ip * v_norm + zip.pp;
        let real_kw = p_kw * zip_multiplier;
        let reactive_base = zip.zq * v_norm * v_norm + zip.iq * v_norm + zip.pq;
        let reactive_kvar = if zip.pf.abs() < 1e-9 {
            0.0
        } else {
            let tan_phi = zip.pf.clamp(-1.0, 1.0).acos().tan();
            real_kw * tan_phi * reactive_base
        };
        (real_kw, reactive_kvar)
    }

    #[test]
    fn apply_is_bit_identical_to_legacy_arithmetic_for_all_class_rows() {
        for name in ALL_CLASS_NAMES {
            let zip = zip_defaults_for_class(name)
                .unwrap_or_else(|| panic!("class table missing row for {name:?}"));
            for &v in &[0.9, 1.0, 1.1] {
                for &p_kw in &[0.372, 1.0, 4.815] {
                    let (real, reactive) = zip.apply(p_kw, v);
                    let (exp_real, exp_reactive) = legacy_apply(&zip, p_kw, v);
                    assert_eq!(
                        real.to_bits(),
                        exp_real.to_bits(),
                        "{name}: real power diverged at p={p_kw} v={v}: {real} vs {exp_real}"
                    );
                    assert_eq!(
                        reactive.to_bits(),
                        exp_reactive.to_bits(),
                        "{name}: reactive diverged at p={p_kw} v={v}: {reactive} vs {exp_reactive}"
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

    #[test]
    fn reactive_kvar_equals_p_times_tan_acos_pf_at_nominal() {
        // With a unity reactive base (0, 0, 1) at nominal voltage the Q-only
        // path reduces exactly to P·tan(acos(pf)).
        let pf = 0.84;
        let zip = ZipLoad::reactive_only(0.0, 0.0, 1.0, pf);
        let p_kw = 3.6;
        let expected = p_kw * pf.acos().tan();
        assert_eq!(zip.reactive_kvar(p_kw, 1.0), expected);

        // Class rows have reactive coefficients summing to ~1, so at nominal
        // voltage Q ≈ P·tan(acos(pf)) within floating-point roundoff.
        for name in ALL_CLASS_NAMES {
            let zip = zip_defaults_for_class(name).expect("row");
            if zip.pf.abs() < 1e-9 || (zip.pf - 1.0).abs() < 1e-9 {
                continue;
            }
            let q = zip.reactive_kvar(p_kw, 1.0);
            let expected = p_kw * zip.pf.clamp(-1.0, 1.0).acos().tan();
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
        assert_eq!(zip.reactive_kvar(5.0, 1.05), 0.0);
        assert_eq!(zip.apply(5.0, 1.05).1, 0.0);
        // constant_power() carries the sentinel by construction.
        assert_eq!(ZipLoad::constant_power().reactive_kvar(5.0, 1.0), 0.0);
    }

    #[test]
    fn pf_unity_produces_exactly_zero_reactive() {
        // acos(1.0) == 0.0 and tan(0.0) == 0.0 exactly, so Q is exactly zero
        // even with nonzero reactive coefficients (e.g. the Lighting row).
        let zip = zip_defaults_for_class("Lighting").expect("row");
        assert_eq!(zip.pf, 1.0);
        assert_eq!(zip.tan_phi(), 0.0);
        assert_eq!(zip.reactive_kvar(2.0, 1.1), 0.0);
        assert_eq!(zip.apply(2.0, 1.1).1, 0.0);
    }

    #[test]
    fn reactive_only_leaves_real_power_untouched_at_all_voltages() {
        let zip = ZipLoad::reactive_only(14.78, -23.71, 9.93, 0.84);
        assert_eq!((zip.zp, zip.ip, zip.pp), (0.0, 0.0, 1.0));
        for &v in &[0.9, 1.0, 1.1] {
            let p_kw = 2.75;
            let (real, reactive) = zip.apply(p_kw, v);
            assert_eq!(real.to_bits(), p_kw.to_bits(), "real changed at v={v}");
            assert_eq!(reactive, zip.reactive_kvar(p_kw, v));
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
        assert_eq!(zip.reactive_kvar(2.5, 0.0), 0.0);
        assert_eq!(zip.reactive_kvar(0.0, 1.0), 0.0);
        assert_eq!(zip.reactive_kvar(0.0, 0.0), 0.0);
        // apply() and reactive_kvar() agree at v=0.
        assert_eq!(zip.apply(2.5, 0.0).1, zip.reactive_kvar(2.5, 0.0));
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
            assert_eq!(zip.reactive_kvar(3.2, 1.05), 0.0);
            assert_eq!(zip.apply(3.2, 0.95).0.to_bits(), 3.2_f64.to_bits());
        }
    }
}
