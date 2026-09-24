# stream-filter.sh: edge cases for CI pipeline input processing
**Review ID**: scr-07
**Category**: scripts
**Date**: 2026-05-26

## Files Reviewed
scripts/stream-filter.sh

## Vendor/Reference Files Consulted
None

## Findings
### Finding 1: [Severity: medium] Binary data / null bytes silently truncated on output
**Description**: Line 28 unconditionally echoes every input line to stdout via `printf '%s\n' "$line"` before any JSON validation. Bash stores variables as null-terminated C strings; if the input contains null bytes, `$line` truncates at the first null byte. The truncated data is then written to stdout without warning. For a CI pipeline tool that may receive corrupted binary data (e.g., a crashed build step dumping a core dump to stderr), this causes silent data loss.
**Code Location**: `scripts/stream-filter.sh:28`
**Root Cause**: `read -r` on line 27 and `printf '%s\n'` on line 28 both silently truncate at null bytes. No detection or guard exists for non-text input.
**Impact**: Corrupted or binary input produces truncated output on stdout. Downstream consumers of the JSONL log file would receive malformed lines without any indication of the problem. No diagnostic message is emitted on stderr.

### Finding 2: [Severity: medium] No guard against extremely long input lines
**Description**: The `while IFS= read -r line` loop on line 27 has no maximum line-length cap (no `-N` flag). While bash's `read` can handle moderately long lines, a single input line approaching or exceeding available memory (e.g., a multi-gigabyte JSON blob with no newline, or a corrupted stream) would cause the script to consume unbounded memory and potentially trigger OOM. The script's own usage in `verify-tickets.sh:311` pipes `claude 2>&1` into it, meaning a runaway claude process could emit an unterminated line.
**Code Location**: `scripts/stream-filter.sh:27`
**Root Cause**: `read` without `-N` or an explicit line-length guard (`${#line}` check) provides no buffer control.
**Impact**: OOM risk on large or corrupted input. In CI environments with memory limits, this could cause the entire pipeline job to be killed.

### Finding 3: [Severity: medium] Exit code always 0 even when all parsing fails
**Description**: Every jq invocation uses the pattern `var="$(...)" || continue` or `var="$(...)" || var=default`. This means jq failures are always suppressed, and the while loop exits with the status of its last command (which is typically 0 since the `case` statement and arithmetic expressions succeed). The script exits 0 regardless of whether it successfully processed any JSON or whether every single line was unparseable garbage. The caller cannot distinguish successful processing from total failure.
**Code Location**: `scripts/stream-filter.sh:32` (and all similar jq-pipe chains at lines 37,39,45,48-49,53-54,58,64-65,94,97-98,102-103,106-107,111,113,141-143)
**Root Cause**: Error-suppression idioms (`|| continue`, `|| varname=""`, `2>/dev/null`) discard all failure information. No counter or summary is accumulated to inform the exit code.
**Impact**: A missing or broken `jq` binary (after line 28's passthrough) produces zero human-readable output but exits 0. The calling script in `verify-tickets.sh:312` (`| tee "${log_file}"; then ... SUCCESS=$((SUCCESS + 1))`) counts this as a success. Silent failures go undetected.

### Finding 4: [Severity: medium] Merged stderr from upstream process causes non-JSON passthrough
**Description**: The script's documented usage (`verify-tickets.sh:310-311`) pipes `claude ... 2>&1` into it, meaning both stdout (stream-json) and stderr (human-readable diagnostics) are merged on stdin. Non-JSON stderr lines pass through unconditionally to stdout via line 28, interleaved with valid JSONL. While this is arguably a feature for raw log capture, it means the "raw JSONL" on stdout is not guaranteed to be valid JSONL — downstream consumers that expect strictly valid JSON may fail.
**Code Location**: `scripts/stream-filter.sh:28` combined with the `2>&1` merge in `verify-tickets.sh:310`
**Root Cause**: No separation of the stdout and stderr streams. A more robust approach would be to pass only stdout to the filter while redirecting stderr separately, or to detect and route non-JSON lines to stderr or a separate output.
**Impact**: The log file at `${log_file}` can contain non-JSON diagnostic lines intermixed with valid JSONL entries. Consumers that parse this file as JSONL (e.g., `jq -s . log.jsonl`) will fail on the non-JSON lines.

### Finding 5: [Severity: low] Inefficient per-line jq process spawning
**Description**: For each assistant/user input line, the script spawns 5–10+ separate jq processes (e.g., lines 45, 48, 49, 53, 54, 58, 64, 65 for the `assistant` case). Each invocation creates a subshell and a pipe via `printf ... | jq ...`. While the memory footprint is low (true streaming), the CPU overhead from process creation is proportional to input volume. For claude stream-json (typically ≤100 lines), this is negligible; for repurposing as a general CI log filter, it could become a bottleneck.
**Code Location**: `scripts/stream-filter.sh:45-74` (assistant block), `94-137` (user block)
**Root Cause**: JSON parsing is not batched — each field extraction is a separate jq invocation rather than combining multiple extractions into a single jq call.
**Impact**: High-latency processing per line. Not a practical problem at current usage volumes, but limits the script's general applicability.

### Finding 6: [Severity: low] Shebang uses `/usr/bin/env bash` with no fallback
**Description**: The shebang on line 1 is `#!/usr/bin/env bash`. This relies on `env` being present at `/usr/bin/env`, which is standard on macOS and most Linux distributions but may not exist in minimal container images (e.g., `scratch`, `busybox`, or distroless images). The script uses bash-specific features (`set -o pipefail`, `((...))` arithmetic, `read -r`, `${var:offset:length}` substring expansion) so `#!/bin/bash` is required over `#!/bin/sh`.
**Code Location**: `scripts/stream-filter.sh:1`
**Root Cause**: Portability assumption about `/usr/bin/env` availability. The rest of the HARES scripts consistently use `#!/usr/bin/env bash` (verify-tickets.sh, review-all.sh, check-si-guard.sh), so this is consistent with the project's convention.
**Impact**: Script fails with "not found" in containers lacking `/usr/bin/env`. Low severity because the project consistently uses this pattern and the script targets CI pipelines on standard runner environments.

## Summary
- Total findings: 6
- Critical: 0
- High: 0
- Medium: 4
- Low: 2

## Recommendations
1. **Add a null-byte guard** before line 28: detect null bytes in `$line` with a simple check (e.g., `[[ "$line" = *$'\0'* ]]`) and either skip the line with a warning to stderr or strip null bytes before output.
2. **Add a maximum line-length guard**: after `read`, check `${#line}` against a reasonable maximum (e.g., 10 MB) and truncate or reject lines exceeding the limit, emitting a warning to stderr.
3. **Track parse failures** with a counter or boolean flag. If no line was successfully parsed (meaning all jq invocations failed), exit non-zero (e.g., exit 2) so callers can detect silent failures.
4. **Separate stderr handling** in verify-tickets.sh: instead of `claude ... 2>&1 | stream-filter.sh | tee log.jsonl`, use process substitution or separate descriptors to pipe only claude's stdout to stream-filter.sh, while capturing stderr separately (e.g., to a dedicated diagnostics log).
5. **Batch jq processing** (optional/optimization): combine multiple `.field // empty` extractions into a single jq invocation per line (e.g., `jq '{type, subtype, model}'`) to reduce process spawning overhead.
6. **Document jq dependency** prominently: add a comment near line 1 noting that `jq` is required. The script currently has no explicit dependency check.

## References / Citations
- Bash manual: `read` builtin line-length limits and null-byte behavior (GNU Bash Reference Manual §4.2)
- Bash manual: `set -e` interaction with `&&`/`||` lists and compound commands (GNU Bash Reference Manual §4.3.1)
- POSIX.1-2017: null bytes in shell variables are not supported; `$var` expansion truncates at `\0` (IEEE Std 1003.1-2017, Vol. 1 §2.6.3)
- Usage context: `scripts/verify-tickets.sh:307-318` demonstrates the `2>&1` merge pattern and exit code dependency
