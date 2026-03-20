use std::sync::Arc;

use chrono::{Datelike, Timelike};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use rand_distr::{Distribution, StandardNormal};

use crate::{DomainId, EnvironmentState, HaresError};

/// Canonical custom-domain id used for schedule payloads in `EnvironmentState.custom_domains`.
pub const SCHEDULE_DOMAIN_ID: DomainId = DomainId(u16::MAX);

/// Returns the canonical schedule custom-domain id.
pub const fn schedule_domain_id() -> DomainId {
    SCHEDULE_DOMAIN_ID
}

/// Out-of-range index behavior for schedule-backed sources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundaryPolicy {
    /// Clamp indices to the first/last valid sample.
    Clamp,
    /// Wrap indices modulo the source length.
    Wrap,
    /// Return an error when the index is out-of-bounds.
    Error,
}

/// A lazily-evaluated source for schedule values.
#[non_exhaustive]
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum ScheduleSource {
    /// Fixed scalar value.
    Constant(f64),
    /// 24-hour weekday/weekend profile with monthly scaling and maximum value.
    DailyProfile {
        weekday: [f64; 24],
        weekend: [f64; 24],
        month_multipliers: [f64; 12],
        max_value: f64,
    },
    /// Column index in the schedule custom-domain payload.
    ColumnRef {
        col_idx: usize,
        boundary: BoundaryPolicy,
    },
    /// Solar-aware profile scaled by monthly multipliers and maximum value.
    SolarAware {
        daytime_fraction: f64,
        evening_fraction: f64,
        overnight_fraction: f64,
        month_multipliers: [f64; 12],
        max_value: f64,
        dusk_altitude_threshold_deg: f64,
    },
    /// Stateful pseudo-random source, deterministic by `seed` + call order.
    SeededNoise {
        base: f64,
        std_dev: f64,
        seed: [u8; 32],
        draw_count: u64,
        rng: ChaCha8Rng,
    },
    /// Shared data with cursor-based advancement.
    Shared {
        data: Arc<[f64]>,
        cursor: usize,
        boundary: BoundaryPolicy,
    },
}

impl PartialEq for ScheduleSource {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Constant(a), Self::Constant(b)) => a == b,
            (
                Self::DailyProfile {
                    weekday: a_wd,
                    weekend: a_we,
                    month_multipliers: a_mm,
                    max_value: a_max,
                },
                Self::DailyProfile {
                    weekday: b_wd,
                    weekend: b_we,
                    month_multipliers: b_mm,
                    max_value: b_max,
                },
            ) => a_wd == b_wd && a_we == b_we && a_mm == b_mm && a_max == b_max,
            (
                Self::ColumnRef {
                    col_idx: a_idx,
                    boundary: a_boundary,
                },
                Self::ColumnRef {
                    col_idx: b_idx,
                    boundary: b_boundary,
                },
            ) => a_idx == b_idx && a_boundary == b_boundary,
            (
                Self::SolarAware {
                    daytime_fraction: a_day,
                    evening_fraction: a_eve,
                    overnight_fraction: a_overnight,
                    month_multipliers: a_mm,
                    max_value: a_max,
                    dusk_altitude_threshold_deg: a_dusk,
                },
                Self::SolarAware {
                    daytime_fraction: b_day,
                    evening_fraction: b_eve,
                    overnight_fraction: b_overnight,
                    month_multipliers: b_mm,
                    max_value: b_max,
                    dusk_altitude_threshold_deg: b_dusk,
                },
            ) => {
                a_day == b_day
                    && a_eve == b_eve
                    && a_overnight == b_overnight
                    && a_mm == b_mm
                    && a_max == b_max
                    && a_dusk == b_dusk
            }
            (
                Self::SeededNoise {
                    base: a_base,
                    std_dev: a_std,
                    seed: a_seed,
                    draw_count: a_count,
                    rng: _,
                },
                Self::SeededNoise {
                    base: b_base,
                    std_dev: b_std,
                    seed: b_seed,
                    draw_count: b_count,
                    rng: _,
                },
            ) => a_base == b_base && a_std == b_std && a_seed == b_seed && a_count == b_count,
            (
                Self::Shared {
                    data: a_data,
                    cursor: a_cursor,
                    boundary: a_boundary,
                },
                Self::Shared {
                    data: b_data,
                    cursor: b_cursor,
                    boundary: b_boundary,
                },
            ) => a_data == b_data && a_cursor == b_cursor && a_boundary == b_boundary,
            _ => false,
        }
    }
}

impl ScheduleSource {
    /// Resolve the current value from this source.
    pub fn value_at(&mut self, env: &EnvironmentState) -> Result<f64, HaresError> {
        match self {
            Self::Constant(v) => Ok(*v),
            Self::DailyProfile {
                weekday,
                weekend,
                month_multipliers,
                max_value,
            } => {
                let hour = env.current_time.hour() as usize;
                let month_idx = env.current_time.month0() as usize;
                let is_weekend = env.current_time.weekday().num_days_from_monday() >= 5;
                let frac = if is_weekend {
                    weekend[hour]
                } else {
                    weekday[hour]
                };
                Ok(frac * month_multipliers[month_idx] * *max_value)
            }
            Self::ColumnRef { col_idx, boundary } => {
                let payload = env
                    .custom_domains
                    .iter()
                    .find(|d| d.domain_id == SCHEDULE_DOMAIN_ID)
                    .and_then(|d| d.custom_payload.as_ref())
                    .ok_or_else(|| {
                        HaresError::Equipment(
                            "schedule domain payload not found in environment custom domains"
                                .to_string(),
                        )
                    })?;
                let idx = resolve_index(*col_idx as i64, payload.len(), *boundary)?;
                Ok(payload[idx])
            }
            Self::SolarAware {
                daytime_fraction,
                evening_fraction,
                overnight_fraction,
                month_multipliers,
                max_value,
                dusk_altitude_threshold_deg,
            } => {
                let hour = env.current_time.hour() as usize;
                let month_idx = env.current_time.month0() as usize;
                let sun_alt = env.weather.solar_altitude_deg;

                let frac = if sun_alt > *dusk_altitude_threshold_deg {
                    *daytime_fraction
                } else if hour >= 5 {
                    *evening_fraction
                } else {
                    *overnight_fraction
                };

                Ok(frac * month_multipliers[month_idx] * *max_value)
            }
            Self::SeededNoise {
                base,
                std_dev,
                seed: _,
                draw_count,
                rng,
            } => {
                let z: f64 = StandardNormal.sample(rng);
                *draw_count = draw_count.checked_add(1).expect("draw_count overflow");
                Ok(*base + *std_dev * z)
            }
            Self::Shared {
                data,
                cursor,
                boundary,
            } => {
                let idx = resolve_index(*cursor as i64, data.len(), *boundary)?;
                let value = data[idx];
                *cursor = cursor.saturating_add(1);
                Ok(value)
            }
        }
    }

    /// Reset any internal mutable state (for checkpoint restart consistency).
    pub fn reset(&mut self) {
        match self {
            Self::SeededNoise {
                base: _,
                std_dev: _,
                seed,
                draw_count,
                rng,
            } => {
                *rng = ChaCha8Rng::from_seed(*seed);
                *draw_count = 0;
            }
            Self::Shared {
                data: _,
                cursor,
                boundary: _,
            } => {
                *cursor = 0;
            }
            Self::Constant(_)
            | Self::DailyProfile { .. }
            | Self::ColumnRef { .. }
            | Self::SolarAware { .. } => {}
        }
    }
}

fn resolve_index(raw_idx: i64, len: usize, boundary: BoundaryPolicy) -> Result<usize, HaresError> {
    if len == 0 {
        return Err(HaresError::Equipment(
            "schedule source data is empty".to_string(),
        ));
    }

    let len_i64 = len as i64;
    let idx = match boundary {
        BoundaryPolicy::Clamp => raw_idx.clamp(0, len_i64 - 1),
        BoundaryPolicy::Wrap => raw_idx.rem_euclid(len_i64),
        BoundaryPolicy::Error => {
            if raw_idx < 0 || raw_idx >= len_i64 {
                return Err(HaresError::Equipment(format!(
                    "schedule index out of bounds: idx={} len={}",
                    raw_idx, len
                )));
            }
            raw_idx
        }
    };

    Ok(idx as usize)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::{TimeZone, Utc};
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    use crate::{DomainUpdate, ZoneId, test_utils::default_env};

    use super::{BoundaryPolicy, SCHEDULE_DOMAIN_ID, ScheduleSource};

    #[test]
    fn constant_returns_constant() {
        let env = default_env();
        let mut source = ScheduleSource::Constant(7.25);
        assert_eq!(
            source.value_at(&env).expect("constant should resolve"),
            7.25
        );
    }

    #[test]
    fn daily_profile_varies_by_hour_weekday_and_month() {
        let mut env = default_env();
        let mut weekday = [0.0; 24];
        weekday[14] = 1.5;
        let mut weekend = [0.0; 24];
        weekend[14] = 0.75;
        let mut month = [1.0; 12];
        month[0] = 2.0;
        month[1] = 0.5;

        let mut source = ScheduleSource::DailyProfile {
            weekday,
            weekend,
            month_multipliers: month,
            max_value: 10.0,
        };

        env.current_time = Utc
            .with_ymd_and_hms(2026, 1, 14, 14, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("weekday value"), 30.0);

        env.current_time = Utc
            .with_ymd_and_hms(2026, 1, 17, 14, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("weekend value"), 15.0);

        // 2026-02-10 is a Tuesday — exercises the weekday+month_multiplier path.
        env.current_time = Utc
            .with_ymd_and_hms(2026, 2, 10, 14, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("month value"), 7.5);
    }

    #[test]
    fn column_ref_reads_schedule_domain_payload() {
        let mut env = default_env();
        env.custom_domains = vec![DomainUpdate {
            domain_id: SCHEDULE_DOMAIN_ID,
            zone_temperatures_c: vec![(ZoneId(1), 21.0)],
            custom_payload: Some(vec![2.0, 4.0, 6.0]),
        }];

        let mut source = ScheduleSource::ColumnRef {
            col_idx: 1,
            boundary: BoundaryPolicy::Clamp,
        };

        assert_eq!(source.value_at(&env).expect("column value"), 4.0);
    }

    #[test]
    fn boundary_policy_wrap_wraps() {
        let env = default_env();
        let data: Arc<[f64]> = Arc::from(vec![10.0, 20.0, 30.0]);
        let mut source = ScheduleSource::Shared {
            data,
            cursor: 5,
            boundary: BoundaryPolicy::Wrap,
        };

        assert_eq!(source.value_at(&env).expect("wrapped value"), 30.0);
    }

    #[test]
    fn boundary_policy_error_errors_when_out_of_bounds() {
        let env = default_env();
        let data: Arc<[f64]> = Arc::from(vec![10.0, 20.0]);
        let mut source = ScheduleSource::Shared {
            data,
            cursor: 5,
            boundary: BoundaryPolicy::Error,
        };

        let err = source
            .value_at(&env)
            .expect_err("out-of-bounds should error");
        assert!(
            err.to_string().contains("out of bounds"),
            "expected out-of-bounds error, got: {err}"
        );
    }

    #[test]
    fn seeded_noise_is_deterministic_given_seed() {
        let env = default_env();
        let seed = [7_u8; 32];

        let mut a = ScheduleSource::SeededNoise {
            base: 5.0,
            std_dev: 1.2,
            seed,
            draw_count: 0,
            rng: ChaCha8Rng::from_seed(seed),
        };
        let mut b = ScheduleSource::SeededNoise {
            base: 5.0,
            std_dev: 1.2,
            seed,
            draw_count: 0,
            rng: ChaCha8Rng::from_seed(seed),
        };

        let mut values = Vec::with_capacity(8);
        for _ in 0..8 {
            let va = a.value_at(&env).expect("value a");
            let vb = b.value_at(&env).expect("value b");
            assert_eq!(va, vb);
            values.push(va);
        }

        // reset() must rewind the RNG to the beginning of the sequence.
        a.reset();
        assert_eq!(
            a.value_at(&env).expect("value a after reset"),
            values[0],
            "reset() did not rewind to the first draw"
        );
    }

    #[test]
    fn shared_cursor_advances() {
        let env = default_env();
        let data: Arc<[f64]> = Arc::from(vec![1.0, 2.0, 3.0]);
        let mut source = ScheduleSource::Shared {
            data,
            cursor: 0,
            boundary: BoundaryPolicy::Clamp,
        };

        assert_eq!(source.value_at(&env).expect("first"), 1.0);
        assert_eq!(source.value_at(&env).expect("second"), 2.0);
        assert_eq!(source.value_at(&env).expect("third"), 3.0);
        assert_eq!(source.value_at(&env).expect("clamped"), 3.0);
    }
}
