#!/usr/bin/env bash
# Block commits that hardcode credentials in JSON files.
#
# Rules (both enforced):
#   1. Any "api_key" field must be an env-var reference of the form "${VAR}".
#      Anything else (literal string, empty, etc.) is rejected.
#   2. Known credential literals anywhere in the file are rejected:
#        - AIzaSy... (Google/Gemini)
#        - sk-ant-... (Anthropic)
#        - sk-... (OpenAI)
#
# Usage:
#   check_no_credentials.sh            # scans git-staged JSON files
#   check_no_credentials.sh FILE ...   # scans the given files
#
# Exit status: 0 if clean, 1 if any violation.

set -euo pipefail

if [ "$#" -gt 0 ]; then
    files=("$@")
else
    mapfile -t files < <(git diff --cached --name-only --diff-filter=ACM | grep -E '\.json$' || true)
fi

if [ "${#files[@]}" -eq 0 ]; then
    exit 0
fi

fail=0

for f in "${files[@]}"; do
    [ -f "$f" ] || continue

    # Rule 1: api_key must reference ${VAR}.
    # Capture every line matching "api_key": "<value>", then reject any whose
    # value is not wrapped as ${...}.
    while IFS= read -r match; do
        line_no="${match%%:*}"
        line="${match#*:}"
        if ! printf '%s' "$line" | grep -qE '"api_key"[[:space:]]*:[[:space:]]*"\$\{[^}]+\}"'; then
            printf 'ERROR: %s:%s  api_key is not an env placeholder\n' "$f" "$line_no"
            printf '       %s\n' "$line"
            fail=1
        fi
    done < <(grep -nE '"api_key"[[:space:]]*:[[:space:]]*"[^"]*"' "$f" || true)

    # Rule 2: known credential literals anywhere in the file.
    matches=$(grep -nE 'AIzaSy[0-9A-Za-z_-]{33}|sk-ant-[A-Za-z0-9_-]{20,}|sk-[A-Za-z0-9]{20,}' "$f" || true)
    if [ -n "$matches" ]; then
        printf 'ERROR: %s contains credential-like literals:\n' "$f"
        printf '%s\n' "$matches" | sed 's/^/       /'
        fail=1
    fi
done

if [ "$fail" -ne 0 ]; then
    printf '\nCommit blocked. Replace hardcoded credentials with "${VAR}" references.\n'
    printf 'Re-run manually with: bash scripts/check_no_credentials.sh [FILE ...]\n'
    exit 1
fi

exit 0
