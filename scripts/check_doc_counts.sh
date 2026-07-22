#!/usr/bin/env bash
# CI drift guard: fail if canonical tool/guide counts in the docs no longer
# match the code. Counts are derived from source by derive_tool_counts.sh, then
# checked against a small set of PINNED canonical doc assertions. When a tool is
# added/removed, the code count changes, the pinned doc line stops matching, and
# CI fails — forcing the doc to be updated in the same PR.
#
# This intentionally checks only a few authoritative lines (the toolkit-package
# table in §40 and the §41 index headers), not every count everywhere — enough
# to catch the drift that actually recurred in the 2026-07 docs audit.
#
# Usage:  scripts/check_doc_counts.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# shellcheck disable=SC1090
eval "$(scripts/derive_tool_counts.sh --env)"

fail=0
# Args: <description> <file> <expected-substring>
assert_contains() {
  local desc="$1" file="$2" needle="$3"
  if grep -qF -- "$needle" "$file"; then
    echo "  ok   $desc"
  else
    echo "  FAIL $desc"
    echo "       expected to find in $file:"
    echo "         \"$needle\""
    fail=1
  fi
}

echo "Canonical counts from code: gsheets=$GSHEETS_ALIAS_TOOLS gdocs=$GDOCS_ALIAS_TOOLS gdocsread=$GDOCSREAD_ALIAS_TOOLS gdocs_total=$GDOCS_TOTAL_TOOLS guides=$DEV_GUIDES"
echo "Checking pinned doc assertions:"

PKG=docs/developer_guide/40_toolkit_packages.md
IDX=docs/developer_guide/41_builtin_tools_index.md

assert_contains "40: gsheets package row"   "$PKG" "| \`gsheets\` | $GSHEETS_ALIAS_TOOLS |"
assert_contains "40: gdocs package row"     "$PKG" "| \`gdocs\` | $GDOCS_ALIAS_TOOLS |"
assert_contains "40: gdocsread package row" "$PKG" "| \`gdocsread\` | $GDOCSREAD_ALIAS_TOOLS |"
assert_contains "41: gdocs section header"  "$IDX" "## gdocs ($GDOCS_TOTAL_TOOLS tools)"

if [[ "$fail" -ne 0 ]]; then
  echo ""
  echo "Doc counts are out of sync with the code. Update the docs above (and run"
  echo "scripts/derive_tool_counts.sh to see all current counts), then re-run."
  exit 1
fi
echo "All pinned doc counts match the code."
