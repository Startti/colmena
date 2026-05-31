#!/usr/bin/env bash
# Hexagonal architecture guard for src/libs/colmena/src/documents/.
#
# Rules enforced:
#   - domain/ MUST NOT import application/ or infrastructure/
#   - application/ (except runtime.rs) MUST NOT import infrastructure/
#   - runtime.rs is the only composition root that imports adapters concretely
#
# See: docs/superpowers/specs/2026-05-30-html-documents-module-design.md §3.5

set -euo pipefail

ROOT="src/libs/colmena/src/documents"
fail=0

violations=$(grep -rn "use crate::documents::application" "${ROOT}/domain/" 2>/dev/null || true)
if [ -n "${violations}" ]; then
  echo "❌ domain imports application:"
  echo "${violations}"
  fail=1
fi

violations=$(grep -rn "use crate::documents::infrastructure" "${ROOT}/domain/" 2>/dev/null || true)
if [ -n "${violations}" ]; then
  echo "❌ domain imports infrastructure:"
  echo "${violations}"
  fail=1
fi

# application files except runtime.rs cannot import infrastructure
for f in $(find "${ROOT}/application" -name '*.rs' -not -name 'runtime.rs' 2>/dev/null); do
  # Exclude #[cfg(test)] mod tests — tests are allowed to use infra adapters
  # (Tests typically construct real LocalFsAssetStore, ExcelRenderer, etc. for verification).
  # We strip lines inside test modules by deleting from `#[cfg(test)]` to EOF, then grep.
  src=$(awk '/^#\[cfg\(test\)\]/{exit} {print}' "${f}")
  if echo "${src}" | grep -qn "use crate::documents::infrastructure" ; then
    echo "❌ application file ${f} imports infrastructure outside tests (only runtime.rs may):"
    echo "${src}" | grep -n "use crate::documents::infrastructure"
    fail=1
  fi
done

if [ $fail -ne 0 ]; then
  echo ""
  echo "Hexagonal layer violations found. See spec §3.5."
  exit 1
fi

echo "✅ Hexagonal architecture compliance OK."
