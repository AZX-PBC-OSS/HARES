# Boiler `flow_rate_kg_s` and `return_temp_c` Silent Defaults

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-io/hpxml/resolve_hvac

## Problem

`crates/hares-io/src/hpxml/resolve_hvac.rs:639,643,680,684` boiler resolution uses silent defaults:
- `flow_rate_kg_s = .unwrap_or(0.5)`
- `return_temp_c = .unwrap_or(40.0)`

Both are reasonable order-of-magnitude values for residential hydronic systems, but neither is cited and both are silently substituted when the HPXML file omits the data. Boiler part-load efficiency depends sensitively on return water temperature (condensing boilers especially: efficiency drops 5-10% as return temp crosses the dewpoint of the flue gas). A wrong return_temp default biases annual heating energy.

## Current Behavior

`crates/hares-io/src/hpxml/resolve_hvac.rs:639,643,680,684`:
```rust
let flow_rate_kg_s = params.get("flow_rate_kg_s").copied().unwrap_or(0.5);
let return_temp_c = params.get("return_temp_c").copied().unwrap_or(40.0);
```

No diagnostic on the substitution. No citation for the default values.

## Required Behavior

Choose one for each:

A. **Cite and log** — keep the default but add inline citation (e.g. ASHRAE HoF 2021 Ch. 36 "Hydronic Heating and Cooling" Table 5 — typical residential flow rate and return temperatures) and emit `tracing::debug!` when the default is taken.

B. **Error loudly** — return `HpxmlError::MissingField` when the value is absent; require the input to supply both.

Recommended path: A with citations, since residential HPXML files commonly omit hydronic loop details and the user expects a sensible default. The debug log must include the equipment ID and the substituted value.

## Approach

1. Open `resolve_hvac.rs` at the four cited line numbers.
2. For each silent default:
   - Add `tracing::debug!(equipment_id = ..., field = "flow_rate_kg_s", value = 0.5, "boiler flow rate not specified; using ASHRAE typical default");` (analogous for return_temp_c).
   - Add an inline comment citing ASHRAE HoF 2021 Ch. 36 Table 5 (or the appropriate primary source — verify chapter/table numbers).
3. Add a unit test asserting the debug message fires and the substituted value is consumed.
4. Document the defaults in the boiler config doc-comment so users know what is being assumed.

## Definition of Done

- [ ] `flow_rate_kg_s` default 0.5 cited inline at `resolve_hvac.rs:639,680`
- [ ] `return_temp_c` default 40.0 cited inline at `resolve_hvac.rs:643,684`
- [ ] `tracing::debug!` emitted when each default is taken
- [ ] Unit test asserts debug message and substituted value
- [ ] Boiler config doc-comment documents the defaults

## Verification

```bash
cargo test -p hares-io resolve_hvac boiler
cargo test -p hares-io hpxml_parity
```

## References

- ASHRAE Handbook of Fundamentals 2021 Ch. 36 "Hydronic Heating and Cooling" — typical residential boiler flow rates and return temperatures (verify table numbers).
- ASHRAE Handbook HVAC Systems and Equipment 2020 Ch. 32 "Boilers" — condensing boiler efficiency vs return water temperature.
- HPXML Specification v4.x §8.4 "Heating Systems" — boiler-specific extension fields.

## Related Tickets

- 079-propane-oil-furnace-boiler-unsupported-parse-error
- 105-default-hp-lockout-temp-citation
