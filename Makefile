.PHONY: check-no-magic-config

# Fail if any built-in equipment init() uses raw config accessors.
# Custom equipment (Python adapter layer) is excluded from this scan.
# Exit code 0 from rg = matches found = FAIL.
# Exit code 1 from rg = no matches = PASS.
# Any other exit code (rg runtime error) = FAIL to prevent false passes.
#
# NOTE: This target will fail until CFG-015 migrates all built-in equipment to
# typed config structs. Wire it into CI only after that migration is complete.
check-no-magic-config:
	@rg --type rust \
	    'config\.get_f64\(|config\.get_str\(|config\.get_bool\(|first_f64\(' \
	    crates/hares-equipment/src/hvac/ \
	    crates/hares-equipment/src/water_heater/ \
	    crates/hares-equipment/src/battery/ \
	    crates/hares-equipment/src/ev/ \
	    crates/hares-equipment/src/pv/ \
	    crates/hares-equipment/src/generator.rs \
	    crates/hares-equipment/src/ventilation.rs \
	    --glob '!*/tests/*'; \
	exit_code=$$?; \
	if [ "$$exit_code" -eq 0 ]; then \
	    echo "FAIL: magic-string config access found in built-in equipment"; \
	    exit 1; \
	elif [ "$$exit_code" -eq 1 ]; then \
	    echo "PASS: no magic-string config access in built-in equipment"; \
	else \
	    echo "FAIL: rg returned unexpected exit code $$exit_code"; \
	    exit 1; \
	fi
