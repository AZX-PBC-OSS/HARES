.PHONY: check-no-magic-config

# Fail if any built-in equipment `init*()` method body uses raw config accessors.
# Custom equipment (Python adapter layer) is excluded from this scan.
# The scan is intentionally conservative: false negatives are acceptable, but
# false positives in non-init helpers must be avoided.
# Exit code 0 from rg = matches found = FAIL.
# Exit code 1 from rg = no matches = PASS.
# Any other exit code (rg runtime error) = FAIL to prevent false passes.
check-no-magic-config:
	@tmp_file=$$(mktemp); \
	trap 'rm -f "$$tmp_file"' EXIT; \
	files=$$(rg --files \
	    crates/hares-equipment/src/hvac/ \
	    crates/hares-equipment/src/water_heater/ \
	    crates/hares-equipment/src/battery/ \
	    crates/hares-equipment/src/ev/ \
	    crates/hares-equipment/src/pv/ \
	    crates/hares-equipment/src/generator.rs \
	    crates/hares-equipment/src/ventilation.rs \
	    --glob '!*/tests/*'); \
	for file in $$files; do \
	    awk ' \
	        function brace_delta(line,    tmp, open_count, close_count) { \
	            tmp = line; \
	            open_count = gsub(/\{/, "{", tmp); \
	            tmp = line; \
	            close_count = gsub(/\}/, "}", tmp); \
	            return open_count - close_count; \
	        } \
	        BEGIN { capture = 0; seen_brace = 0; depth = 0 } \
	        { \
	            if (!capture && $$0 ~ /^[[:space:]]*(pub[[:space:]]+)?fn (init|init_[[:alnum:]_]+)\(/) { \
	                capture = 1; \
	                seen_brace = 0; \
	                depth = 0; \
	            } \
	            if (capture) { \
	                print FILENAME ":" FNR ":" $$0; \
	                if (index($$0, "{") > 0) { \
	                    seen_brace = 1; \
	                } \
	                if (seen_brace) { \
	                    depth += brace_delta($$0); \
	                    if (depth <= 0) { \
	                        capture = 0; \
	                        seen_brace = 0; \
	                        depth = 0; \
	                    } \
	                } \
	            } \
	        } \
	    ' "$$file" >> "$$tmp_file"; \
	done; \
	rg 'config\.get_f64\(|config\.get_str\(|config\.get_bool\(|first_f64\(' "$$tmp_file"; \
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
