#!/usr/bin/env python3
"""Check that documentation references under docs/ point at files that exist.

Two checks run:

1. Every relative markdown link resolves to a real file.
2. Every `tests/graphs/**.json` path named in a LIVING doc exists. Graph paths are
   usually written as inline code spans rather than links, so check 1 never sees
   them, yet a doc naming a graph nobody can run is just as broken.

Check 1 skips fenced code blocks and inline code spans, so a `[link](url)` shown as
an example of markdown syntax is not reported as a real link. External links
(http/https/mailto) and pure anchors (#foo) are out of scope.

A `path#anchor` link is resolved by its path part; a `path:LINE` suffix is
tolerated because docs use it as a clickable line reference.

`docs/superpowers/`, `docs/history/` and `docs/archive/` are historical records:
plans there legitimately name graphs that were proposed and never created, so
check 2 skips them. Their markdown links are still checked.

Exit code 1 if anything is broken, so this can gate CI.
"""
import os
import re
import sys

LINK = re.compile(r'(?<!\!)\[[^\]]*\]\(([^)\s]+?)(?:\s+"[^"]*")?\)')
FENCE = re.compile(r'^\s*(```|~~~)')
INLINE_CODE = re.compile(r'`[^`]*`')
LINE_SUFFIX = re.compile(r':L?\d+(?:-L?\d+)?$')
GRAPH_REF = re.compile(r'tests/graphs/[A-Za-z0-9_/]*\.json')

# Docs that record what was planned or shipped in the past, not what exists today.
HISTORICAL = ("docs/superpowers", "docs/history", "docs/archive")

# A doc may deliberately name a graph that does not exist yet. Each entry must say
# why, so the exception stays reviewable instead of silently absorbing regressions.
GRAPH_REF_ALLOWLIST = {
    # BACKLOG proposes creating this graph; it is a to-do, not a claim it exists.
    "tests/graphs/agents/gsheets_overwrite_e2e.json",
}

def targets(path):
    """Yield (lineno, raw_target) for links outside fenced code blocks."""
    in_fence = False
    with open(path, encoding="utf-8", errors="replace") as fh:
        for lineno, line in enumerate(fh, 1):
            if FENCE.match(line):
                in_fence = not in_fence
                continue
            if in_fence:
                continue
            # A link shown inside backticks is sample syntax, not a real link.
            line = INLINE_CODE.sub("", line)
            for m in LINK.finditer(line):
                yield lineno, m.group(1)

def resolve(doc_dir, target):
    """Return the on-disk path a relative link points at, or None to skip."""
    if target.startswith(("http://", "https://", "mailto:", "#")):
        return None
    path = target.split("#", 1)[0]
    path = LINE_SUFFIX.sub("", path)
    if not path:
        return None
    return os.path.normpath(os.path.join(doc_dir, path))

def graph_refs(path):
    """Yield (lineno, graph_path) for every tests/graphs/*.json path named in a doc."""
    with open(path, encoding="utf-8", errors="replace") as fh:
        for lineno, line in enumerate(fh, 1):
            for m in GRAPH_REF.finditer(line):
                yield lineno, m.group(0)

def main(roots):
    broken, checked = [], 0
    for root in roots:
        for dirpath, _dirnames, filenames in os.walk(root):
            for name in sorted(filenames):
                if not name.endswith(".md"):
                    continue
                doc = os.path.join(dirpath, name)
                for lineno, target in targets(doc):
                    dest = resolve(dirpath, target)
                    if dest is None:
                        continue
                    checked += 1
                    if not os.path.exists(dest):
                        broken.append((doc, lineno, target))

    missing_graphs, graphs_checked = [], 0
    for root in roots:
        for dirpath, _dirnames, filenames in os.walk(root):
            if dirpath.startswith(HISTORICAL):
                continue
            for name in sorted(filenames):
                if not name.endswith((".md", ".json")):
                    continue
                doc = os.path.join(dirpath, name)
                for lineno, graph in graph_refs(doc):
                    if graph in GRAPH_REF_ALLOWLIST:
                        continue
                    graphs_checked += 1
                    if not os.path.exists(graph):
                        missing_graphs.append((doc, lineno, graph))

    for doc, lineno, target in broken:
        print(f"{doc}:{lineno}: broken link -> {target}")
    for doc, lineno, graph in missing_graphs:
        print(f"{doc}:{lineno}: doc names a graph that does not exist -> {graph}")

    print(f"\n{checked} relative links checked, {len(broken)} broken.")
    print(f"{graphs_checked} graph references checked, {len(missing_graphs)} missing.")
    return 1 if (broken or missing_graphs) else 0

if __name__ == "__main__":
    sys.exit(main(sys.argv[1:] or ["docs"]))
