#!/usr/bin/env python3
"""Report the review size of the current candidate BEFORE `gentle-ai review start`.

The review tier is decided by total changed lines, and documentation counts
toward that total just like code does (verified empirically against the review
store: `original_changed_lines == code + docs` for every recorded lineage).
Since docs run roughly 25-30% of a change in this repo, a 300-line code change
routinely lands at 450+ total and buys the 4-lens tier without anyone noticing.

Run this before freezing a candidate, so the tier is a decision instead of a
surprise.

Size and correction budget are exact: backtested against all 15 lineages
recorded in the local review store, `changed_lines` and `correction_budget`
matched gentle-ai 2.1.11 in every case.

The TIER is a FLOOR, never a ceiling. gentle-ai raises it on signals that have
nothing to do with size — observed first-hand on this very script: a new file
gaining mode 100755 raises `executable_mode`, and a file that spawns
subprocesses raises `process_boundary`, either of which forces `high` and its
four lenses at any line count. This tool reports the ones it can detect and
says so; treat an unflagged `medium` as "at least medium", not as a promise.

Usage:
    python3 scripts/review_size.py                  # uncommitted work vs HEAD
    python3 scripts/review_size.py --base-ref origin/develop   # fetch first
    python3 scripts/review_size.py --json
"""

from __future__ import annotations

import argparse
import json
import math
import os
import subprocess
import sys

# A change at or below this many total lines stays out of the 4-lens tier.
HIGH_TIER_LINES = 400

# Paths whose presence forces the high tier regardless of size. Matched against
# non-documentation paths only: a guide *about* OAuth is not a hot path.
HOT_PATH_MARKERS = (
    "secure_value",
    "secure_suspend",
    "/secrets",
    "auth",
    "credential",
    "oauth",
    "/sql",
    "permission",
)


def git(*args: str) -> str:
    result = subprocess.run(
        ["git", *args], capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        sys.exit(f"git {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


def is_documentation(path: str) -> bool:
    return path.startswith("docs/") or path.endswith(".md")


# Content markers for the `process_boundary` signal: a file that spawns
# processes forces the high tier regardless of size.
PROCESS_MARKERS = (
    "subprocess",
    "os.system",
    "os.exec",
    "Popen",
    "std::process::Command",
    "shell=True",
)


def mode_transitions(*diff_args: str) -> dict[str, tuple[str, str]]:
    """Map path -> (old_mode, new_mode) for tracked changes, via `git diff --raw`.

    Untracked files never appear here; the caller supplies "000000" for them.
    """
    transitions: dict[str, tuple[str, str]] = {}
    for line in git("diff", "--raw", *diff_args).splitlines():
        if not line.startswith(":"):
            continue
        meta, _, path = line.partition("\t")
        fields = meta[1:].split()
        if len(fields) < 2 or not path:
            continue
        transitions[path] = (fields[0], fields[1])
    return transitions


def escalating_signals(
    paths: list[str], transitions: dict[str, tuple[str, str]]
) -> tuple[list[str], list[str]]:
    """Non-size signals that force the high tier, plus paths we could not read.

    Best effort, not exhaustive — the binary applies rules this cannot see.
    """
    signals: list[str] = []
    skipped: list[str] = []
    for path in paths:
        if is_documentation(path):
            continue
        if path in transitions:
            old_mode, new_mode = transitions[path]
        else:
            # Not in the diff: either genuinely new (untracked, so absent from
            # the index) or tracked and unchanged. Only the former is "gaining".
            indexed = git("ls-files", "-s", "--", path).split()
            old_mode = indexed[0] if indexed else "000000"
            new_mode = "100755" if os.access(path, os.X_OK) else "100644"
        # The signal is GAINING the bit, not merely having it. Flagging every
        # already-executable file would force `high` on any edit to a script.
        if new_mode == "100755" and old_mode != "100755":
            signals.append(f"executable_mode: {path} ({old_mode} -> {new_mode})")
        try:
            with open(path, encoding="utf-8", errors="ignore") as handle:
                body = handle.read()
        except OSError:
            skipped.append(path)
            continue
        if any(marker in body for marker in PROCESS_MARKERS):
            signals.append(f"process_boundary: {path}")
    return signals, skipped


def is_test(path: str) -> bool:
    return (
        path.startswith("tests/")
        or "/tests/" in path
        or path.endswith("_test.py")
        or path.startswith("python/tests/")
    )


def numstat(*diff_args: str) -> list[tuple[int, str]]:
    """Return (changed_lines, path) pairs, skipping binary entries."""
    entries: list[tuple[int, str]] = []
    for line in git("diff", "--numstat", *diff_args).splitlines():
        parts = line.split("\t")
        if len(parts) < 3 or parts[0] == "-":
            continue
        entries.append((int(parts[0]) + int(parts[1]), parts[2]))
    return entries


def untracked() -> tuple[list[tuple[int, str]], list[str]]:
    """Untracked files are part of the candidate — gentle-ai freezes them too."""
    entries: list[tuple[int, str]] = []
    skipped: list[str] = []
    for path in git(
        "ls-files", "--others", "--exclude-standard"
    ).splitlines():
        try:
            with open(path, "rb") as handle:
                blob = handle.read()
        except OSError:
            skipped.append(path)
            continue
        if b"\0" in blob:  # binary
            continue
        # An empty file is zero lines; a file with no trailing newline still
        # ends a line. `count("\n") or 1` got both of these wrong.
        lines = 0 if not blob else blob.count(b"\n") + (
            0 if blob.endswith(b"\n") else 1
        )
        entries.append((lines, path))
    return entries, skipped


def collect(
    base_ref: str | None,
) -> tuple[list[tuple[int, str]], dict[str, tuple[str, str]], list[str]]:
    """Return (entries, mode transitions, unreadable paths)."""
    diff_args = (git("merge-base", base_ref, "HEAD").strip(),) if base_ref else ("HEAD",)
    entries = numstat(*diff_args)
    transitions = mode_transitions(*diff_args)
    extra, skipped = untracked()
    return entries + extra, transitions, skipped


def classify(code: int, docs: int, hot: list[str]) -> tuple[str, int, str]:
    """Return (tier, lens_count, reason). `hot` carries every high-tier signal."""
    total = code + docs
    if hot:
        return "high", 4, f"forced high by: {'; '.join(sorted(set(hot)))}"
    if total > HIGH_TIER_LINES:
        if code == 0:
            return "medium", 1, "pure documentation above the size threshold"
        return "high", 4, f"{total} total lines exceeds {HIGH_TIER_LINES}"
    if code == 0:
        return "low", 0, "pure documentation below the size threshold"
    return "medium", 1, f"{total} total lines is within {HIGH_TIER_LINES}"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--base-ref",
        help="compare against merge-base with this ref (e.g. develop) "
        "instead of uncommitted work vs HEAD",
    )
    parser.add_argument(
        "--json", action="store_true", help="emit machine-readable output"
    )
    args = parser.parse_args()

    entries, transitions, unreadable = collect(args.base_ref)
    if not entries:
        print("No changes in the candidate — nothing to size.")
        return 0

    docs = sum(n for n, p in entries if is_documentation(p))
    tests = sum(n for n, p in entries if is_test(p) and not is_documentation(p))
    code = sum(n for n, p in entries if not is_documentation(p))
    total = code + docs
    hot = [
        f"hot path: {p}"
        for _, p in entries
        if not is_documentation(p) and any(m in p for m in HOT_PATH_MARKERS)
    ]
    signals, more_unreadable = escalating_signals(
        [p for _, p in entries], transitions
    )
    hot += signals
    unreadable += more_unreadable

    tier, lenses, reason = classify(code, docs, hot)
    budget = min(200, math.ceil(total / 2))
    margin = budget / total * 100 if total else 0.0

    if args.json:
        print(
            json.dumps(
                {
                    "total": total,
                    "code": code,
                    "docs": docs,
                    "tests": tests,
                    "tier": tier,
                    "lenses": lenses,
                    "reason": reason,
                    "correction_budget": budget,
                    "correction_margin_pct": round(margin, 1),
                    "hot_paths": sorted(set(hot)),
                    "unreadable_paths": sorted(set(unreadable)),
                },
                indent=2,
            )
        )
        return 0

    print(f"  total          {total:>6}   lines in the candidate")
    print(f"    code         {code:>6}   ({f'{tests} in tests' if tests else 'no test lines'})")
    docs_share = f"{docs / total * 100:.0f}% of the candidate" if total else "no line delta"
    print(f"    docs         {docs:>6}   ({docs_share})")
    print()
    print(f"  tier           {tier:>6}   -> {lenses} review lens(es)")
    print(f"  reason         {reason}")
    print(f"  budget         {budget:>6}   correction lines ({margin:.0f}% margin)")

    if unreadable:
        print()
        print(f"  WARNING: {len(unreadable)} path(s) could not be read and are")
        print("  NOT counted above — the total is a lower bound, not exact:")
        for path in sorted(set(unreadable)):
            print(f"    - {path}")

    if tier != "high":
        print()
        print("  Tier is a FLOOR: gentle-ai may still raise it on signals this")
        print("  tool cannot see. Never read an unflagged verdict as a promise.")

    if tier == "high":
        over = total - HIGH_TIER_LINES
        print()
        if hot:
            print("  A non-size signal forces the 4-lens tier; slicing will not lower it:")
            for signal in sorted(set(hot)):
                print(f"    - {signal}")
        else:
            print(f"  {over} lines over the threshold. Cutting that much drops this")
            print("  to 1 lens. Use the `chained-pr` skill to slice it.")
        if margin < 30:
            print(
                f"  Correction margin is only {margin:.0f}% — the budget saturates at 200,"
            )
            print("  so a large candidate has little room to absorb findings before")
            print("  it escalates. Escalation has no reentry: you re-review from zero.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
