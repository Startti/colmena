#!/usr/bin/env bash
#
# scripts/check_python_env.sh
#
# Validates that the local .venv Python interpreter matches the Python
# version that the dag_engine binary was linked against via PyO3.
#
# Why this matters
# ----------------
# PyO3 embeds a specific Python interpreter at link time. Setting PYTHONPATH
# only changes WHERE Python looks for packages — it cannot change the ABI of
# the running interpreter. If your .venv runs Python 3.9 but the binary
# embeds Python 3.14, loading numpy fails with the confusing
#   "you should not try to import numpy from its source directory"
# message because numpy's compiled `_multiarray_umath.so` is built for one
# ABI and the running interpreter expects another.
#
# This script catches that mismatch BEFORE you waste time chasing phantom
# numpy/pandas errors during smoke tests. Run it any time you create or
# rebuild the .venv, or after upgrading the Homebrew Python that PyO3 may
# have picked up.
#
# Exit codes:
#   0 — venv and binary Python ABI match (or close enough to be compatible)
#   1 — mismatch detected; instructions printed for recovery
#   2 — environment not set up (no .venv, no binary, no Python)
#
# Usage:
#   scripts/check_python_env.sh
#
# Run from the repo root.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

red()    { printf "\033[31m%s\033[0m\n" "$*"; }
green()  { printf "\033[32m%s\033[0m\n" "$*"; }
yellow() { printf "\033[33m%s\033[0m\n" "$*"; }
bold()   { printf "\033[1m%s\033[0m\n" "$*"; }

bold "[check_python_env] Verifying .venv Python ↔ dag_engine binary alignment"
echo

# 1) Confirm .venv exists
if [ ! -x ".venv/bin/python" ]; then
  red "❌ No .venv found at ./venv/bin/python"
  echo "   Create one matching your binary's Python:"
  echo "     /opt/homebrew/opt/python@3.14/bin/python3.14 -m venv .venv"
  echo "     .venv/bin/pip install 'numpy>=2.1' pandas scipy openpyxl"
  exit 2
fi
VENV_VER=$(.venv/bin/python -c 'import sys; print(f"{sys.version_info.major}.{sys.version_info.minor}")')
VENV_FULL=$(.venv/bin/python --version 2>&1 | awk '{print $2}')
echo "  venv interpreter: .venv/bin/python   ($VENV_FULL)"

# 2) Confirm dag_engine binary exists
BIN=""
for candidate in target/debug/dag_engine target/release/dag_engine; do
  if [ -x "$candidate" ]; then
    BIN="$candidate"
    break
  fi
done
if [ -z "$BIN" ]; then
  red "❌ No dag_engine binary found (target/debug or target/release)."
  echo "   Build it first:  cargo build --bin dag_engine"
  exit 2
fi
echo "  binary:           $BIN"

# 3) Inspect what Python lib the binary is linked against
if ! command -v otool >/dev/null 2>&1; then
  yellow "⚠️  otool not available (non-macOS?) — can't inspect linked Python version."
  yellow "    On Linux, run: ldd $BIN | grep -i python"
  exit 0
fi
BIN_PY_LINE=$(otool -L "$BIN" 2>/dev/null | grep -iE "python.framework/Versions/" | head -1 || true)
if [ -z "$BIN_PY_LINE" ]; then
  yellow "⚠️  Binary not dynamically linked to a Python framework — likely static build."
  yellow "    Skipping ABI check (assumed safe)."
  exit 0
fi
# Extract version from e.g. "/opt/.../Versions/3.14/Python"
BIN_VER=$(echo "$BIN_PY_LINE" | sed -nE 's|.*/Versions/([0-9]+\.[0-9]+)/Python.*|\1|p')
echo "  binary embeds Python:  $BIN_VER"
echo "  binary link line:      $(echo "$BIN_PY_LINE" | awk '{print $1}')"
echo

# 4) Compare
if [ "$VENV_VER" = "$BIN_VER" ]; then
  green "✅ Match: .venv ($VENV_FULL) is the same Python series as the binary ($BIN_VER)."
  # 5) Sanity-load pandas to surface any package-side issues immediately
  if .venv/bin/python -c 'import pandas, numpy, scipy' 2>/dev/null; then
    green "✅ pandas/numpy/scipy import cleanly in the venv."
  else
    yellow "⚠️  Python versions match but pandas/numpy/scipy don't import cleanly."
    yellow "    Reinstall:  .venv/bin/pip install --force-reinstall 'numpy>=2.1' pandas scipy"
    exit 1
  fi
  exit 0
fi

red "❌ ABI MISMATCH"
echo "   .venv runs Python  $VENV_VER  ($VENV_FULL)"
echo "   binary embeds      $BIN_VER"
echo
echo "  Setting PYTHONPATH to .venv site-packages will fail at numpy import"
echo "  with the misleading \"source directory\" error. Fix one of:"
echo
echo "  (A) Rebuild venv to match the binary (recommended):"
echo "        rm -rf .venv .venv.bak; mv .venv .venv.bak 2>/dev/null || true"
echo "        /opt/homebrew/opt/python@${BIN_VER}/bin/python${BIN_VER} -m venv .venv"
echo "        .venv/bin/pip install 'numpy>=2.1' pandas scipy openpyxl"
echo
echo "  (B) Rebuild colmena against your venv Python:"
echo "        PYO3_PYTHON=\$(pwd)/.venv/bin/python cargo build --bin dag_engine"
echo
exit 1
