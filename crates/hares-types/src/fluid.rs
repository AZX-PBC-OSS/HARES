//! Fluid-domain shared state payload types.

use serde::{Deserialize, Serialize};

use crate::{FluidType, HaresError, LoopId};

/// Fluid loop state resolved each timestep by the fluid domain solver.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FluidLoopState {
    pub loop_id: LoopId,
    pub fluid_type: FluidType,
    pub net_power_w: f64,
    pub mean_supply_temp_c: f64,
    pub mean_return_temp_c: f64,
}

/// Encode/decode `Vec<FluidLoopState>` to/from `Vec<f64>` for `DomainUpdate::custom_payload`.
///
/// Layout per loop:
/// `[loop_id as f64, fluid_type as f64, net_power_w, mean_supply_temp_c, mean_return_temp_c]`.
pub struct FluidDomainPayload;

impl FluidDomainPayload {
    #[must_use]
    pub fn encode(states: &[FluidLoopState]) -> Option<Vec<f64>> {
        if states.is_empty() {
            return None;
        }
        let mut payload = Vec::with_capacity(states.len() * 5);
        for state in states {
            payload.push(f64::from(state.loop_id.0));
            payload.push(fluid_type_to_f64(state.fluid_type));
            payload.push(state.net_power_w);
            payload.push(state.mean_supply_temp_c);
            payload.push(state.mean_return_temp_c);
        }
        Some(payload)
    }

    pub fn decode(payload: &[f64]) -> Result<Vec<FluidLoopState>, HaresError> {
        if !payload.len().is_multiple_of(5) {
            return Err(HaresError::Envelope(format!(
                "invalid fluid payload length {}, expected multiple of 5",
                payload.len()
            )));
        }
        let mut states = Vec::with_capacity(payload.len() / 5);
        for chunk in payload.chunks_exact(5) {
            let loop_id = f64_to_loop_id(chunk[0])?;
            let fluid_type = f64_to_fluid_type(chunk[1])?;
            states.push(FluidLoopState {
                loop_id,
                fluid_type,
                net_power_w: chunk[2],
                mean_supply_temp_c: chunk[3],
                mean_return_temp_c: chunk[4],
            });
        }
        Ok(states)
    }
}

fn fluid_type_to_f64(fluid_type: FluidType) -> f64 {
    match fluid_type {
        FluidType::Water => 0.0,
        FluidType::Glycol => 1.0,
        FluidType::Refrigerant => 2.0,
    }
}

fn f64_to_fluid_type(value: f64) -> Result<FluidType, HaresError> {
    #[allow(clippy::cast_possible_truncation)]
    match value.round() as i64 {
        0 => Ok(FluidType::Water),
        1 => Ok(FluidType::Glycol),
        2 => Ok(FluidType::Refrigerant),
        _ => Err(HaresError::Envelope(format!(
            "invalid fluid type discriminator {value}"
        ))),
    }
}

fn f64_to_loop_id(value: f64) -> Result<LoopId, HaresError> {
    if !value.is_finite() || value < 0.0 || value > f64::from(u16::MAX) || value.fract() != 0.0 {
        return Err(HaresError::Envelope(format!(
            "invalid loop id value {value}"
        )));
    }
    Ok(LoopId(value as u16))
}

#[cfg(test)]
mod tests {
    use super::{FluidDomainPayload, FluidLoopState};
    use crate::{FluidType, LoopId};

    #[test]
    fn payload_round_trip() {
        let states = vec![
            FluidLoopState {
                loop_id: LoopId(1),
                fluid_type: FluidType::Water,
                net_power_w: 41860.0,
                mean_supply_temp_c: 60.0,
                mean_return_temp_c: 40.0,
            },
            FluidLoopState {
                loop_id: LoopId(2),
                fluid_type: FluidType::Glycol,
                net_power_w: 1200.0,
                mean_supply_temp_c: 45.0,
                mean_return_temp_c: 42.5,
            },
        ];
        let payload = FluidDomainPayload::encode(&states).unwrap();
        let decoded = FluidDomainPayload::decode(&payload).unwrap();
        assert_eq!(decoded, states);
    }

    #[test]
    fn encode_empty_returns_none() {
        assert_eq!(FluidDomainPayload::encode(&[]), None);
    }

    #[test]
    fn decode_rejects_invalid_length() {
        let err = FluidDomainPayload::decode(&[1.0, 0.0, 2.0]).unwrap_err();
        assert!(err.to_string().contains("invalid fluid payload length"));
    }
}
