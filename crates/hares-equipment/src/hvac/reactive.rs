//! Per-component reactive power for HVAC equipment.
//!
//! An HVAC unit's electrical port blends components with physically
//! different power factors: the compressor motor (the dominant reactive
//! source), auxiliary fan/pump motors, and purely resistive elements.
//! Folding one blended power factor over the total draw fabricates
//! reactive power that a real installation never exhibits — most severely
//! `P_ER · tan(acos(0.84)) ≈ 0.646 · P_ER` of phantom kvar while a heat
//! pump's electric-resistance backup runs. HARES therefore computes
//! reactive power per component and sums:
//!
//! ```text
//! Q_total = Σ_c  P_c · tan(acos(pf_c)) · reactive_base_c(V)
//! ```
//!
//! Component assignment (normative; see docs/equipment/power-factor.md):
//!
//! - **Primary component** — the component the equipment's ZIP class row
//!   describes (compressor for DX equipment, blower for a gas furnace,
//!   circulation pump for a gas boiler, resistance element for electric
//!   furnace/boiler/baseboard) — uses the unit ZIP resolved via
//!   [`crate::config::resolve_reactive_zip`] (config sidecar override →
//!   class defaults). A user `"zip"`/`pf` override therefore retargets the
//!   *primary* component, not the whole blend.
//! - **Secondary fan/pump motors** use the fixed physical component ZIPs
//!   below ([`FAN_MOTOR_ZIP`], [`LOOP_PUMP_ZIP`]) via
//!   [`secondary_motor_zip`], which honours the unit-level `pf = 0.0`
//!   "no reactive" sentinel.
//! - **Resistive components** (ER backup elements, crankcase / base-pan
//!   heaters, resistive defrost elements) have pf = 1.0 exactly and
//!   contribute `Q ≡ 0` by construction — they appear in no sum term.

use hares_types::zip::ZipLoad;

/// Indoor blower / outdoor condenser fan motor component (pf 0.87).
///
/// Reactive-only (Rule R1) projection of the OCHRE `fans` ZIP row — the
/// same row `zip_defaults_for_class("Ventilation Fan")` returns, so a
/// ducted blower and a standalone ventilation fan with equal wattage
/// produce identical Q. A drift test below enforces equality with the
/// canonical class table.
pub(crate) const FAN_MOTOR_ZIP: ZipLoad = ZipLoad {
    zp: 0.0,
    ip: 0.0,
    pp: 1.0,
    zq: 0.5,
    iq: 0.62,
    pq: -0.12,
    pf: 0.87,
    v0: 1.0,
};

/// Hydronic / ground-loop circulation pump motor component (pf 0.84).
///
/// Reactive-only (Rule R1) projection of the PUMPS row (Hajagos & Danai
/// 1998) — the same row `zip_defaults_for_class("Gas Boiler")` returns.
/// A drift test below enforces equality with the canonical class table.
pub(crate) const LOOP_PUMP_ZIP: ZipLoad = ZipLoad {
    zp: 0.0,
    ip: 0.0,
    pp: 1.0,
    zq: 14.78,
    iq: -23.71,
    pq: 9.93,
    pf: 0.84,
    v0: 1.0,
};

/// Resolve a secondary motor component's ZIP from the unit-level ZIP.
///
/// Returns `component` unless the unit ZIP carries the `pf = 0.0` "no
/// reactive configured" sentinel (e.g. a `ZipLoad::constant_power()`
/// sidecar override), in which case reactive power is disabled for the
/// whole unit — secondary components included — and
/// [`ZipLoad::constant_power`] is returned. This keeps the sentinel a
/// unit-wide off switch while nonzero-pf overrides retarget only the
/// primary component.
pub(crate) fn secondary_motor_zip(unit_zip: &ZipLoad, component: ZipLoad) -> ZipLoad {
    // Same sentinel test as `ZipLoad::tan_phi`.
    if unit_zip.pf.abs() < 1e-9 {
        ZipLoad::constant_power()
    } else {
        component
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hares_types::zip::zip_defaults_for_class;

    fn reactive_only_projection(class: &str) -> ZipLoad {
        let row = zip_defaults_for_class(class).expect("class row exists");
        ZipLoad::reactive_only(row.zq, row.iq, row.pq, row.pf)
    }

    /// The fan component constant must not drift from the canonical class
    /// table (single source of truth for the OCHRE `fans` row).
    #[test]
    fn fan_motor_zip_matches_class_table() {
        assert_eq!(FAN_MOTOR_ZIP, reactive_only_projection("Ventilation Fan"));
    }

    /// The pump component constant must not drift from the canonical class
    /// table (PUMPS row, shared by the Gas Boiler class).
    #[test]
    fn loop_pump_zip_matches_class_table() {
        assert_eq!(LOOP_PUMP_ZIP, reactive_only_projection("Gas Boiler"));
    }

    /// A unit-level pf=0 sentinel disables secondary components too.
    #[test]
    fn sentinel_disables_secondary_components() {
        let disabled = secondary_motor_zip(&ZipLoad::constant_power(), FAN_MOTOR_ZIP);
        assert_eq!(disabled, ZipLoad::constant_power());
        assert_eq!(disabled.reactive_kvar(1.0, 1.0), 0.0);
    }

    /// A nonzero-pf unit override leaves secondary components at their
    /// physical values (the override retargets the primary component only).
    #[test]
    fn nonzero_pf_override_keeps_secondary_components() {
        let unit = ZipLoad::reactive_only(14.78, -23.71, 9.93, 0.9);
        assert_eq!(secondary_motor_zip(&unit, FAN_MOTOR_ZIP), FAN_MOTOR_ZIP);
        assert_eq!(secondary_motor_zip(&unit, LOOP_PUMP_ZIP), LOOP_PUMP_ZIP);
    }

    /// Q_c = P_c · tan(acos(pf_c)) exactly at nominal voltage.
    #[test]
    fn component_q_over_p_is_tan_acos_pf_at_nominal_voltage() {
        let p = 0.5;
        let fan_q = FAN_MOTOR_ZIP.reactive_kvar(p, 1.0);
        let expected = p * 0.87_f64.acos().tan() * (0.5 + 0.62 - 0.12);
        assert!((fan_q - expected).abs() < 1e-12);
        let pump_q = LOOP_PUMP_ZIP.reactive_kvar(p, 1.0);
        let expected = p * 0.84_f64.acos().tan() * (14.78 - 23.71 + 9.93);
        assert!((pump_q - expected).abs() < 1e-12);
    }
}
