#!/usr/bin/env python3
"""Verifica `status`/`errorText` (aditivos) en frames node-end/subgraph-node-end.

Uso: python3 scripts/verify_node_end_status_e2e.py <archivo.sse> [banderas]
  --closed-error PATH          end en PATH con status=error y errorText no vacío
  --closed-ok PATH             end en PATH SIN clave `status`
  --open PATH                  start en PATH, sin end correspondiente
                                (frontera dejada abierta a propósito)
  --closed-before-tool-output  el/los --closed-error preceden al primer
                                tool-output-available de nivel 0
  --balanced                   todo start tiene exactamente un end (por path)
  --no-frame TYPE               ningún frame de ese type aparece (repetible)
"""
import argparse
import json
import sys
from collections import Counter

STARTS = {"node-start", "subgraph-node-start"}
ENDS = {"node-end", "subgraph-node-end"}


def load(path):
    frames = []
    for line in open(path, encoding="utf-8", errors="replace"):
        line = line.strip()
        if line.startswith("data: "):
            try:
                frames.append(json.loads(line[6:]))
            except json.JSONDecodeError:
                pass
    return frames


class Report:
    def __init__(self):
        self.rows = []

    def check(self, name, ok, detail):
        self.rows.append((name, bool(ok), detail))

    def render(self):
        w = max((len(n) for n, _, _ in self.rows), default=0)
        for name, ok, detail in self.rows:
            print(f"  [{'PASS' if ok else 'FAIL'}] {name.ljust(w)}  {detail}")
        return sum(1 for _, ok, _ in self.rows if not ok)


def at(frames, types, path):
    return [f for f in frames if f.get("type") in types and f.get("path") == path]


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("sse_file")
    for flag in ("--closed-error", "--closed-ok", "--open", "--no-frame"):
        p.add_argument(flag, action="append", default=[], metavar="PATH" if flag != "--no-frame" else "TYPE")
    p.add_argument("--closed-before-tool-output", action="store_true")
    p.add_argument("--balanced", action="store_true")
    args = p.parse_args()

    frames = load(args.sse_file)
    r = Report()

    for path in args.closed_error:
        f = next((e for e in at(frames, ENDS, path) if e.get("status") == "error"), None)
        r.check(f"--closed-error {path}", f and bool(f.get("errorText")),
                 f"status={f.get('status')!r} errorText={f.get('errorText')!r}" if f else "no matching error close")

    for path in args.closed_ok:
        matches = at(frames, ENDS, path)
        r.check(f"--closed-ok {path}", matches and "status" not in matches[0],
                 f"status present={'status' in matches[0]}" if matches else "no end frame found")

    for path in args.open:
        has_start = bool(at(frames, STARTS, path))
        has_end = bool(at(frames, ENDS, path))
        r.check(f"--open {path}", has_start and not has_end, f"start seen={has_start} end seen={has_end}")

    if args.closed_before_tool_output:
        out_idx = next((i for i, f in enumerate(frames)
                         if f.get("type") == "tool-output-available" and f.get("level", 0) == 0), None)
        err_idxs = [i for i, f in enumerate(frames) if f.get("type") in ENDS and f.get("status") == "error"]
        ok = out_idx is not None and err_idxs and all(i < out_idx for i in err_idxs)
        r.check("--closed-before-tool-output", ok, f"error close(s) at {err_idxs}, tool-output-available at {out_idx}")

    if args.balanced:
        starts = Counter(f.get("path") for f in frames if f.get("type") in STARTS)
        ends = Counter(f.get("path") for f in frames if f.get("type") in ENDS)
        mismatched = {p: (n, ends.get(p, 0)) for p, n in starts.items() if n != ends.get(p, 0)}
        r.check("--balanced", not mismatched, "every start has exactly one end" if not mismatched else f"{mismatched}")

    for t in args.no_frame:
        present = [f for f in frames if f.get("type") == t]
        r.check(f"--no-frame {t}", not present, "absent, as expected" if not present else f"found {len(present)}")

    print(f"\n  frames: {len(frames)}\n")
    failed = r.render()
    print()
    print(f"  {failed} verificación(es) FALLARON" if failed else "  todas las verificaciones pasaron")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
