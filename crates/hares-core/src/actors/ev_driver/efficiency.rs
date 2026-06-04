/// Temperature-dependent EV driving efficiency multiplier.
///
/// Returns a multiplier on EPA-rated kWh/mile (> 1.0 = more energy consumed).
/// Piecewise linear model calibrated against fleet-scale data:
/// - AAA 2019 (5 EVs, HVAC on): 41% range loss at -7°C, 17% at 35°C
/// - Geotab 2020 (5.2M trips): 54% rated range at -15°C
/// - DOE/Argonne 2024: 54% range loss at -18°C, 14% at 35°C
/// - Recurrent Auto (30k vehicles): 5% loss at 32°C, 31% at 38°C
///
/// Cold penalty is steeper than hot: cabin heating (resistive 3-6 kW or heat
/// pump 1-3 kW) plus battery internal resistance and preconditioning. Hot
/// weather AC draws 1-2 kW. This curve is fleet-average across heat pump
/// and resistive vehicles per the cited studies.
pub(super) fn temp_efficiency_multiplier(ambient_c: f64) -> f64 {
    const BREAKPOINTS: [(f64, f64); 8] = [
        (-20.0, 2.00), // ~50% range
        (-10.0, 1.61), // ~62% range
        (0.0, 1.33),   // ~75% range
        (10.0, 1.11),  // ~90% range
        (22.0, 1.00),  // EPA baseline
        (30.0, 1.03),  // minimal AC penalty
        (35.0, 1.17),  // AAA 95°F
        (45.0, 1.43),  // extreme heat
    ];

    let t = ambient_c.clamp(BREAKPOINTS[0].0, BREAKPOINTS[BREAKPOINTS.len() - 1].0);
    for i in 0..BREAKPOINTS.len() - 1 {
        let (t0, m0) = BREAKPOINTS[i];
        let (t1, m1) = BREAKPOINTS[i + 1];
        if t <= t1 {
            let frac = (t - t0) / (t1 - t0);
            return m0 + frac * (m1 - m0);
        }
    }
    BREAKPOINTS[BREAKPOINTS.len() - 1].1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_efficiency_multiplier_ranges() {
        let cold = temp_efficiency_multiplier(-10.0);
        assert!(cold > 1.5, "cold should be > 1.5, got {cold}");

        let optimal = temp_efficiency_multiplier(22.0);
        assert!(
            (optimal - 1.0).abs() < 0.01,
            "optimal should be ~1.0, got {optimal}"
        );

        let hot = temp_efficiency_multiplier(35.0);
        assert!(hot > 1.1 && hot < 1.25, "hot should be 1.1-1.25, got {hot}");

        // Monotonic around baseline
        assert!(temp_efficiency_multiplier(15.0) < temp_efficiency_multiplier(5.0));
        assert!(temp_efficiency_multiplier(30.0) < temp_efficiency_multiplier(40.0));
    }
}
