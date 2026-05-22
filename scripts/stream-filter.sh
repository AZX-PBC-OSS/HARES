#!/usr/bin/env bash
# stream-filter.sh
#
# Reads claude --output-format stream-json from stdin.
# Writes raw JSONL to stdout (for tee/log), and writes a human-readable
# summary to stderr (so it appears on the terminal when piped through tee).
#
# Actual stream-json format from claude -p:
#   {"type":"system","subtype":"init",…}
#   {"type":"assistant","message":{"content":[{"type":"text",…},{"type":"tool_use",…}]}}
#   {"type":"user","message":{"content":[{"type":"tool_result","content":"…"}]}}
#   {"type":"result","cost_usd":…}
#
# Usage:
#   claude -p "…" --output-format stream-json | ./scripts/stream-filter.sh 2>&1 | tee log.jsonl

set -euo pipefail

C_YELLOW="\033[33m"
C_GREEN="\033[32m"
C_RED="\033[31m"
C_GRAY="\033[90m"
C_CYAN="\033[36m"
C_BOLD="\033[1m"
C_RESET="\033[0m"

while IFS= read -r line; do
    printf '%s\n' "$line"

    [[ -z "$line" ]] && continue

    event_type="$(printf '%s' "$line" | jq -r '.type // empty' 2>/dev/null)" || continue

    case "${event_type}" in

        system)
            subtype="$(printf '%s' "$line" | jq -r '.subtype // empty' 2>/dev/null)"
            if [[ "${subtype}" == "init" ]]; then
                model="$(printf '%s' "$line" | jq -r '.model // "?"' 2>/dev/null)"
                printf "${C_GRAY}[init model=%s]${C_RESET}\n" "${model}" >&2
            fi
            ;;

        assistant)
            msg="$(printf '%s' "$line" | jq -c '.message // empty' 2>/dev/null)" || continue
            [[ -z "$msg" ]] && continue

            blocks="$(printf '%s' "$msg" | jq -c '.content // []' 2>/dev/null)" || continue
            block_count="$(printf '%s' "$blocks" | jq 'length' 2>/dev/null)" || block_count=0

            idx=0
            while (( idx < block_count )); do
                block="$(printf '%s' "$blocks" | jq -c ".[${idx}]" 2>/dev/null)" || { idx=$((idx+1)); continue; }
                btype="$(printf '%s' "$block" | jq -r '.type // empty' 2>/dev/null)" || { idx=$((idx+1)); continue; }

                case "${btype}" in
                    text)
                        txt="$(printf '%s' "$block" | jq -r '.text // ""' 2>/dev/null)" || txt=""
                        if [[ -n "${txt}" ]]; then
                            printf '%s' "${txt}" >&2
                        fi
                        ;;
                    tool_use)
                        tname="$(printf '%s' "$block" | jq -r '.name // "unknown"' 2>/dev/null)" || tname="unknown"
                        tinput="$(printf '%s' "$block" | jq -c '.input // {}' 2>/dev/null)" || tinput="{}"
                        summary="$(printf '%s' "${tinput}" | jq -r '
                            if .file_path then "\(.file_path)"
                            elif .pattern then "\(.pattern)"
                            elif .command then .command
                            elif .url then .url
                            elif .query then .query
                            elif .keywords then .keywords
                            else "" end
                        ' 2>/dev/null || echo "")"
                        # Truncate
                        if ((${#summary} > 120)); then
                            summary="${summary:0:117}…"
                        fi
                        printf "\n${C_YELLOW}  ▶ %s${C_RESET}" "${tname}" >&2
                        if [[ -n "${summary}" ]]; then
                            printf " ${C_GRAY}%s${C_RESET}" "${summary}" >&2
                        fi
                        printf "\n" >&2
                        ;;
                    thinking)
                        ;;
                esac
                idx=$((idx + 1))
            done
            ;;

        user)
            # Tool results come as type=user with content blocks of type=tool_result
            msg="$(printf '%s' "$line" | jq -c '.message // empty' 2>/dev/null)" || continue
            [[ -z "$msg" ]] && continue

            blocks="$(printf '%s' "$msg" | jq -c '.content // []' 2>/dev/null)" || continue
            block_count="$(printf '%s' "$blocks" | jq 'length' 2>/dev/null)" || block_count=0

            idx=0
            while (( idx < block_count )); do
                block="$(printf '%s' "$blocks" | jq -c ".[${idx}]" 2>/dev/null)" || { idx=$((idx+1)); continue; }
                btype="$(printf '%s' "$block" | jq -r '.type // empty' 2>/dev/null)" || { idx=$((idx+1)); continue; }

                if [[ "${btype}" == "tool_result" ]]; then
                    is_error="$(printf '%s' "$block" | jq -r '.is_error // false' 2>/dev/null)" || is_error="false"
                    content="$(printf '%s' "$block" | jq -r '.content // ""' 2>/dev/null)" || content=""

                    # Also check for tool_use_result with stdout
                    stdout_content=""
                    tool_result_obj="$(printf '%s' "$line" | jq -c '.tool_use_result // empty' 2>/dev/null)" || tool_result_obj=""
                    if [[ -n "${tool_result_obj}" ]]; then
                        stdout_content="$(printf '%s' "${tool_result_obj}" | jq -r '.stdout // ""' 2>/dev/null | head -c 300)" || stdout_content=""
                    fi

                    # Pick the best preview
                    preview="${content}"
                    if [[ -z "${preview}" && -n "${stdout_content}" ]]; then
                        preview="${stdout_content}"
                    fi

                    if [[ "${is_error}" == "true" ]]; then
                        printf "  ${C_RED}✗ error${C_RESET}" >&2
                    else
                        printf "  ${C_GREEN}← ok${C_RESET}" >&2
                    fi

                    # Show a short preview of the result
                    if [[ -n "${preview}" ]]; then
                        # Truncate and sanitize: remove newlines, replace non-ASCII-printable with space
                        preview="$(printf '%.200s' "${preview}" | LC_ALL=C sed 's/[^[:print:]]/ /g')"
                        printf " ${C_GRAY}%.100s${C_RESET}" "${preview}" >&2
                    fi
                    printf "\n" >&2
                fi
                idx=$((idx + 1))
            done
            ;;

        result)
            cost="$(printf '%s' "$line" | jq -r '.cost_usd // ""' 2>/dev/null)" || cost=""
            duration="$(printf '%s' "$line" | jq -r '.duration_ms // ""' 2>/dev/null)" || duration=""
            num_turns="$(printf '%s' "$line" | jq -r '.num_turns // ""' 2>/dev/null)" || num_turns=""
            printf "\n${C_BOLD}── result ──${C_RESET}" >&2
            [[ -n "${cost}" ]] && printf "  cost: \$%.4f" "${cost}" >&2
            [[ -n "${duration}" ]] && printf "  duration: %.1fs" "$(echo "scale=1; ${duration} / 1000" | bc 2>/dev/null || echo "?")" >&2
            [[ -n "${num_turns}" ]] && printf "  turns: %s" "${num_turns}" >&2
            printf "\n" >&2
            ;;

        *)
            ;;
    esac
done
