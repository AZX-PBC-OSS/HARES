//! Shared integration-test helpers for EV scenario runs.
//!
//! One home for the charging-isolating charge-day metric so the EV
//! regression suites (ev_thermal_regressions, equipment_id_regressions)
//! cannot drift apart — the pre-fix copies were byte-identical and the
//! metric itself counted heater-only days as charge-days.

/// Scans an EV scenario run's CSV output and returns
/// `(charge_days, cancelled_peak)`:
///
/// - **charge_days** — the number of days on which the pack's stored
///   energy actually rose: any row-to-row increase of the `EV SOC (-)`
///   column within the day. This isolates *charging*: a heater-only day
///   draws real port power while storing nothing (must not count), and
///   driving discharge never appears on the port at all (the vehicle is
///   `Disconnected` while driving). The pre-fix metric — days with > 1 kWh
///   of positive port power — counted both.
/// - **cancelled_peak** — the peak of the driver actor's cumulative
///   `drive_cancelled` counter.
///
/// Verbosity floor: the SOC column is emitted only at
/// `output_verbosity >= 3` (hares-io output columns); every current caller
/// runs verbosity 5. A below-3 caller fails on the `expect` below, which
/// names the requirement rather than panicking on a mystery missing
/// column.
pub fn ev_charge_days_and_peak_cancelled(
    csv_path: &std::path::Path,
    steps_per_day: usize,
) -> (usize, f64) {
    let contents = std::fs::read_to_string(csv_path).expect("read output CSV");
    let mut lines = contents.lines();
    let header: Vec<&str> = lines
        .next()
        .expect("output CSV has a header")
        .split(',')
        .collect();
    let idx_soc = header.iter().position(|c| *c == "EV SOC (-)").expect(
        "output contains 'EV SOC (-)' — the SOC column requires \
                 output_verbosity >= 3",
    );
    let idx_cancelled = header
        .iter()
        .position(|c| *c == "actor:EvDriver:EV:drive_cancelled")
        .expect("output contains 'actor:EvDriver:EV:drive_cancelled'");

    // A rise smaller than this is float noise in the CSV round-trip, not a
    // charge step: the slowest physical charge step (an L1 trickle in deep
    // cold) moves SOC by ~1e-3, three orders above it.
    const RISE_EPSILON: f64 = 1e-6;

    let mut charge_days = 0usize;
    let mut cancelled_peak = 0.0f64;
    let mut day = 0usize;
    let mut day_charged = false;
    let mut prev_soc: Option<f64> = None;
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').collect();
        let soc: f64 = fields[idx_soc]
            .trim()
            .parse()
            .expect("EV SOC column parses as f64");
        if let Some(prev) = prev_soc
            && soc - prev > RISE_EPSILON
        {
            day_charged = true;
        }
        prev_soc = Some(soc);
        if let Ok(cancelled) = fields[idx_cancelled].trim().parse::<f64>() {
            cancelled_peak = cancelled_peak.max(cancelled);
        }
        // Day boundaries: every `steps_per_day` rows. The header was
        // consumed, so row 0 is the first data row.
        day += 1;
        if day.is_multiple_of(steps_per_day) {
            if day_charged {
                charge_days += 1;
            }
            day_charged = false;
        }
    }
    (charge_days, cancelled_peak)
}
