#!/usr/bin/env python3
"""Generate a mechanical module dependency map for the Colmena Rust crate.

Parses every `use crate::...` statement under src/libs/colmena/src and builds a
grep-friendly index of, per module file:
  - Depends on  : the intra-crate modules this file imports.
  - Used by     : the files that import this module (the blast radius).

The map is DERIVED from source, never hand-edited, so it cannot drift. Regenerate
with:  python3 scripts/gen_module_map.py

Output: docs/agent_context/module_dependency_map.md
"""

from __future__ import annotations

import re
from collections import defaultdict
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
SRC_ROOT = REPO_ROOT / "src" / "libs" / "colmena" / "src"
OUT_FILE = REPO_ROOT / "docs" / "agent_context" / "module_dependency_map.md"


def module_path_for(file: Path) -> str:
    """Map a .rs file to its Rust module path (relative to crate root)."""
    rel = file.relative_to(SRC_ROOT)
    parts = list(rel.parts)
    stem = parts[-1][:-3]  # strip .rs
    if stem in ("lib", "main"):
        return ""  # crate root
    if stem == "mod":
        parts = parts[:-1]  # mod.rs => its parent directory
    else:
        parts = parts[:-1] + [stem]
    return "::".join(parts)


def rel(file: Path) -> str:
    return str(file.relative_to(REPO_ROOT))


def main() -> None:
    rs_files = sorted(SRC_ROOT.rglob("*.rs"))

    # module path -> owning file
    module_to_file: dict[str, Path] = {}
    for f in rs_files:
        module_to_file[module_path_for(f)] = f

    known_modules = set(module_to_file)

    # Collect all `use crate::...` targets per file (handles multi-line blocks).
    use_re = re.compile(r"use\s+crate::([a-zA-Z0-9_:]+)")

    depends_on: dict[Path, set[str]] = defaultdict(set)  # file -> module paths
    used_by: dict[str, set[Path]] = defaultdict(set)     # module path -> importer files

    for f in rs_files:
        text = f.read_text(encoding="utf-8", errors="replace")
        # Normalize multi-line `use crate::x::{ ... }` into the base path only;
        # the base path before `{` is enough to resolve the owning module.
        for m in use_re.finditer(text):
            raw = m.group(1)  # e.g. dag_engine::domain::node
            segments = raw.split("::")
            # longest prefix that is a known module = the imported module file
            target = None
            for i in range(len(segments), 0, -1):
                cand = "::".join(segments[:i])
                if cand in known_modules and module_to_file[cand] != f:
                    target = cand
                    break
            if target is None:
                continue
            depends_on[f].add(target)
            used_by[target].add(f)

    # Blast-radius ranking: modules with the most importers are the riskiest to change.
    ranking = sorted(
        known_modules,
        key=lambda mp: (-len(used_by[mp]), mp),
    )

    lines: list[str] = []
    lines.append("# Module Dependency Map (auto-generated)\n")
    lines.append(
        "> **DO NOT EDIT BY HAND.** Regenerate with `python3 scripts/gen_module_map.py`.\n"
        "> Derived from `use crate::...` statements — the intra-crate import graph.\n"
    )
    lines.append(
        "**How to use (for the exploration/spec phase):** before opening files to "
        "assess a change, look up the target file below. **Used by** is its blast "
        "radius — the files that break if you change its public surface. **Depends on** "
        "is what it needs. Start by reading only those, not the whole repo.\n"
    )

    lines.append(f"- Files indexed: **{len(rs_files)}**")
    lines.append(f"- Modules with at least one importer: **{sum(1 for mp in known_modules if used_by[mp])}**\n")

    # Danger zone table
    lines.append("## Blast-radius ranking (change these with the most care)\n")
    lines.append("| Importers | Module | File |")
    lines.append("|---:|---|---|")
    for mp in ranking[:30]:
        n = len(used_by[mp])
        if n == 0:
            break
        f = module_to_file[mp]
        lines.append(f"| {n} | `{mp or '(crate root)'}` | `{rel(f)}` |")
    lines.append("")

    # Per-file detail, grouped by top-level area for scan-ability.
    lines.append("## Per-file dependencies\n")
    by_area: dict[str, list[Path]] = defaultdict(list)
    for f in rs_files:
        area = f.relative_to(SRC_ROOT).parts[0]
        by_area[area].append(f)

    for area in sorted(by_area):
        lines.append(f"### {area}\n")
        for f in sorted(by_area[area]):
            mp = module_path_for(f)
            deps = sorted(depends_on.get(f, set()))
            importers = sorted(used_by.get(mp, set()))
            lines.append(f"#### `{rel(f)}`")
            lines.append(f"- Module: `{mp or '(crate root)'}`")
            if importers:
                imp_str = ", ".join(f"`{rel(i)}`" for i in importers)
                lines.append(f"- **Used by ({len(importers)})**: {imp_str}")
            else:
                lines.append("- **Used by (0)**: — (leaf / entrypoint / not imported intra-crate)")
            if deps:
                dep_str = ", ".join(f"`{d}`" for d in deps)
                lines.append(f"- Depends on ({len(deps)}): {dep_str}")
            else:
                lines.append("- Depends on (0): — (no intra-crate imports)")
            lines.append("")

    OUT_FILE.parent.mkdir(parents=True, exist_ok=True)
    OUT_FILE.write_text("\n".join(lines), encoding="utf-8")
    print(f"Wrote {OUT_FILE.relative_to(REPO_ROOT)} ({len(lines)} lines)")


if __name__ == "__main__":
    main()
