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
