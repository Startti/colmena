#!/usr/bin/env bash
# Derive canonical tool/alias/guide counts straight from the source of truth.
#
# Docs (indices, guides) drift from code as tools are added/removed. This
# script extracts the REAL counts so doc edits — and the CI drift check
# (scripts/check_doc_counts.sh) — never guess.
#
# Usage:  scripts/derive_tool_counts.sh          # human-readable report
#         scripts/derive_tool_counts.sh --env     # KEY=VALUE (for CI/sourcing)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STOOLS="$ROOT/src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools"
PKG="$STOOLS/toolkit_packages.rs"

# Count the string literals inside a given alias's `tools: &[ ... ]` block.
# Args: <alias>
alias_tool_count() {
  local alias="$1"
  awk -v alias="\"$alias\"" '
    $0 ~ "alias: " alias {inpkg=1}
    inpkg && /tools: &\[/ {intools=1; next}
    intools && /\]/ {print n; exit}
    intools && /^[[:space:]]*"/ {n++}
  ' "$PKG"
}

GSHEETS_ALIAS=$(alias_tool_count gsheets)
GDOCS_ALIAS=$(alias_tool_count gdocs)
GDOCSREAD_ALIAS=$(alias_tool_count gdocsread)

# Total distinct tool-name constants (superset of what any alias exposes).
GDOCS_TOTAL=$(grep -cE 'pub const .*: &str = "gdocs_' "$STOOLS/gdocs_tools.rs")
GSHEETS_TOTAL=$(grep -cE 'pub const .*: &str = "gsheets_' "$STOOLS/gsheets_tools.rs")
CRDT_TOTAL=$(grep -rhoE '"(crdt_doc_[a-z_]+)"' "$STOOLS"/*.rs | sort -u | wc -l | tr -d ' ')

# Developer guides on disk.
DEV_GUIDES=$(ls "$ROOT"/docs/developer_guide/*.md | wc -l | tr -d ' ')

if [[ "${1:-}" == "--env" ]]; then
  cat <<EOF
GSHEETS_ALIAS_TOOLS=$GSHEETS_ALIAS
GDOCS_ALIAS_TOOLS=$GDOCS_ALIAS
GDOCSREAD_ALIAS_TOOLS=$GDOCSREAD_ALIAS
GDOCS_TOTAL_TOOLS=$GDOCS_TOTAL
GSHEETS_TOTAL_TOOLS=$GSHEETS_TOTAL
CRDT_TOTAL_TOOLS=$CRDT_TOTAL
DEV_GUIDES=$DEV_GUIDES
EOF
else
  cat <<EOF
Canonical counts (source of truth = code):

  gsheets alias tools ......... $GSHEETS_ALIAS   (toolkit_packages.rs)
  gdocs alias tools ........... $GDOCS_ALIAS   (toolkit_packages.rs)
  gdocsread alias tools ....... $GDOCSREAD_ALIAS   (toolkit_packages.rs)
  gdocs_* tool constants ...... $GDOCS_TOTAL   (gdocs_tools.rs)
  gsheets_* tool constants .... $GSHEETS_TOTAL   (gsheets_tools.rs)
  crdt_doc_* tools ............ $CRDT_TOTAL   (crdt_doc_*.rs)
  developer_guide/*.md ........ $DEV_GUIDES   (on disk)
EOF
fi
