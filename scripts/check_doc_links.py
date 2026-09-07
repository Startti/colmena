#!/usr/bin/env python3
"""Check that documentation references under docs/ point at files that exist.

Three checks run:

1. Every relative markdown link resolves to a real file.
2. Every `tests/graphs/**.json` path named in a LIVING doc exists. Graph paths are
   usually written as inline code spans rather than links, so check 1 never sees
   them, yet a doc naming a graph nobody can run is just as broken.
3. No rolling changelog reuses a section number. Sections are referenced by number
   from the BACKLOG and from other entries, so two `## 26.` make every such
   reference ambiguous -- and the reader has no way to tell which was meant. This
   happened: two PRs merged the same day each appended a `## 26.`, and the
   collision sat in `develop` unnoticed because nothing looked.

Check 1 skips fenced code blocks and inline code spans, so a `[link](url)` shown as
an example of markdown syntax is not reported as a real link. It also skips targets
that do not look like a path (no directory separator and no file extension), which
is what an indexing expression in prose such as "recientes[B..N](full)" looks like.
External links (http/https/mailto) and pure anchors (#foo) are out of scope.

A `path#anchor` link is resolved by its path part; a `path:LINE` suffix is
tolerated because docs use it as a clickable line reference.

`docs/superpowers/`, `docs/history/` and `docs/archive/` are historical records:
plans there legitimately name graphs that were proposed and never created, so
check 2 skips them. `docs/qa/` is skipped for the mirror-image reason: the
per-node QA plans name the graph each test case still has to be built from.
Their markdown links are still checked.

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
# A rolling changelog's top-level entries: `## 26. Title`. Only the number matters.
CHANGELOG_SECTION = re.compile(r'^## (\d+)\.')

# Collisions that predate this check, kept as data rather than fixed.
#
# Renumbering a historical entry is not free: sections are cited by number from
# the BACKLOG and from plans, and those citations cannot all be resolved. The
# BACKLOG cites "§24 de CHANGELOG_2026-06.md" three times for a router change,
# and NEITHER section numbered 24 in that file is about the router -- so at
# least one citation is already pointing somewhere unintended, and guessing
# which entry should keep the number would bury that rather than surface it.
#
# The point of this check is to stop NEW collisions, which it does. These five
# stay listed, where anyone can see them, until someone who knows that history
# resolves them.
KNOWN_SECTION_COLLISIONS = {
    ("CHANGELOG_2026-06.md", 22),
    ("CHANGELOG_2026-06.md", 23),
    ("CHANGELOG_2026-06.md", 24),
    ("CHANGELOG_2026-06.md", 25),
    ("CHANGELOG_2026-06.md", 26),
}
# A real relative link carries a directory separator or a file extension. Prose
# sometimes contains bracket-paren pairs that are not links at all -- an indexing
# expression, or a diagram quoted with "" instead of backticks -- and those never
# look like a path. Requiring path shape drops exactly that noise.
PATH_SHAPE = re.compile(r'\.[A-Za-z0-9]{1,8}$')

# Docs that record what was planned or shipped in the past, not what exists today.
HISTORICAL = ("docs/superpowers", "docs/history", "docs/archive")

# Docs that describe work still to be done. The per-node QA plans under docs/qa/
# name the graph each test case needs so QA can build it; naming a graph there is
# the assignment, not a claim that it already exists. Check 2 skips them for the
# same reason it skips HISTORICAL -- the graph path is prospective, not a promise.
# Their markdown links are still checked.
PROSPECTIVE = ("docs/qa",)

# A doc may deliberately name a graph that does not exist -- either not yet, or
# not any more. Each entry must say why, so the exception stays reviewable
# instead of silently absorbing regressions.
GRAPH_REF_ALLOWLIST = {
    # BACKLOG proposes creating this graph; it is a to-do, not a claim it exists.
    "tests/graphs/agents/gsheets_overwrite_e2e.json",
    # Deleted, not missing: broken at four independent points (ledger finding
    # #66) and superseded by `trip_planner_v2.json`. The docs that still name it
    # are records of past work -- the 2026-08 changelog entry that found the
    # defects, and the ledger row that tracked them. Rewriting either to remove
    # the name would erase what was found; the graph is gone, the finding is not.
    "tests/graphs/advanced/trip_planner.json",
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
    if "/" not in path and not PATH_SHAPE.search(path):
        return None
    return os.path.normpath(os.path.join(doc_dir, path))

def graph_refs(path):
    """Yield (lineno, graph_path) for every tests/graphs/*.json path named in a doc."""
    with open(path, encoding="utf-8", errors="replace") as fh:
        for lineno, line in enumerate(fh, 1):
            for m in GRAPH_REF.finditer(line):
                yield lineno, m.group(0)

def duplicate_sections(path):
    """Section numbers used more than once in one changelog, with their lines."""
    seen, dupes = {}, []
    with open(path, encoding="utf-8") as handle:
        in_fence = False
        for lineno, line in enumerate(handle, 1):
            if FENCE.match(line):
                in_fence = not in_fence
                continue
            if in_fence:
                continue
            match = CHANGELOG_SECTION.match(line)
            if not match:
                continue
            number = int(match.group(1))
            if number in seen:
                dupes.append((lineno, number, seen[number]))
            else:
                seen[number] = lineno
    return dupes


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
            if dirpath.startswith(HISTORICAL) or dirpath.startswith(PROSPECTIVE):
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

    collisions, changelogs = [], 0
    for root in roots:
        for dirpath, _dirnames, filenames in os.walk(root):
            for name in sorted(filenames):
                if not (name.startswith("CHANGELOG_") and name.endswith(".md")):
                    continue
                doc = os.path.join(dirpath, name)
                changelogs += 1
                for lineno, number, first in duplicate_sections(doc):
                    if (name, number) in KNOWN_SECTION_COLLISIONS:
                        continue
                    collisions.append((doc, lineno, number, first))

    for doc, lineno, target in broken:
        print(f"{doc}:{lineno}: broken link -> {target}")
    for doc, lineno, graph in missing_graphs:
        print(f"{doc}:{lineno}: doc names a graph that does not exist -> {graph}")

    for doc, lineno, number, first in collisions:
        print(f"{doc}:{lineno}: section {number} is already used at line {first}")

    print(f"\n{checked} relative links checked, {len(broken)} broken.")
    print(f"{graphs_checked} graph references checked, {len(missing_graphs)} missing.")
    print(f"{changelogs} changelog(s) checked, {len(collisions)} duplicate section number(s).")
    return 1 if (broken or missing_graphs or collisions) else 0

if __name__ == "__main__":
    sys.exit(main(sys.argv[1:] or ["docs"]))
