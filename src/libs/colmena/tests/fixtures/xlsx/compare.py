#!/usr/bin/env python3
"""Compares what the converter made (a JSON dump from the `dump_a_directory` harness)
with what openpyxl reads back from the same workbook, cell by cell.

    python3 compare.py WORKBOOK.xlsx DUMP.json

Prints one line per difference and exits 1 if there is any. The rules it applies are the
converter's: the first row with a value is the header, sheets with no value or no
chart-free cells are not tables, blank rows are dropped, an empty string is null, a number
is a number (a text column shows it as Excel would), a date is a date or a timestamp, and
an error is its text.
"""
import datetime as dt
import json
import sys

import openpyxl


def canon_expected(v):
    if v is None or v == "":
        return None
    if isinstance(v, bool):
        return ("bool", v)
    if isinstance(v, (int, float)):
        # A whole number beyond 2**53 is exact in the converter only if the file spells
        # it out; a double (what Excel, openpyxl and LibreOffice write) is not, and is
        # compared as a double.
        return ("num", float(v))
    if isinstance(v, dt.datetime):
        return ("ts", v.replace(microsecond=0))
    if isinstance(v, dt.date):
        return ("ts", dt.datetime(v.year, v.month, v.day))
    if isinstance(v, dt.time):
        return ("time", v.replace(microsecond=0))
    return ("str", str(v))


def canon_got(text, typ):
    if text is None:
        return None
    if typ == "int":
        return ("num", float(int(text)))
    if typ == "float":
        return ("num", float(text))
    if typ == "bool":
        return ("bool", text == "true")
    if typ in ("date", "timestamp"):
        t = text.replace("T", " ")
        t = t if " " in t else t + " 00:00:00"
        return ("ts", dt.datetime.strptime(t.split(".")[0], "%Y-%m-%d %H:%M:%S"))
    return ("str", text)


def text_like(c):
    """A cell as the text a string column shows it."""
    if c is None:
        return None
    kind, v = c
    if kind == "num":
        return ("str", str(int(v)) if v == int(v) and abs(v) < 1e15 else repr(v))
    if kind == "int":
        return ("str", str(v))
    if kind == "bool":
        return ("str", "TRUE" if v else "FALSE")
    if kind == "ts":
        return ("str", v.strftime("%Y-%m-%d") if v.time() == dt.time(0) else v.strftime("%Y-%m-%d %H:%M:%S"))
    return c


def main(path, dump_path):
    dump = json.load(open(dump_path))
    if "error" in dump:
        print(f"REFUSED: {dump['error']}")
        return 1
    wb = openpyxl.load_workbook(path, data_only=True)
    bad = 0
    by_sheet = {t["sheet"]: t for t in dump["tables"]}
    skipped = {s["sheet"] for s in dump["skipped"]}
    for ws in wb.worksheets:
        grid = [[canon_expected(c.value) for c in row] for row in ws.iter_rows()]
        grid = [r for r in grid if any(c is not None for c in r)]
        if not grid:
            if ws.title in by_sheet:
                print(f"MISMATCH {ws.title}: a table for an empty sheet")
                bad += 1
            continue
        if ws.title not in by_sheet:
            # Skipped for want of a header, or named differently by the manifest.
            if not any(s.startswith(ws.title[:20]) for s in skipped):
                print(f"MISMATCH {ws.title}: no table and not skipped")
                bad += 1
            continue
        t = by_sheet[ws.title]
        width = len(t["columns"])
        header, data = grid[0], grid[1:]
        want_names = [str(c[1]) if c else "" for c in header[:width]]
        got_names = [c["name"] for c in t["columns"]]
        for w, g in zip(want_names, got_names):
            if w and w != g and not g.startswith(w.strip()):
                print(f"MISMATCH {ws.title}: header {w!r} read as {g!r}")
                bad += 1
        if len(data) != len(t["rows"]):
            print(f"MISMATCH {ws.title}: {len(data)} data rows expected, {len(t['rows'])} made")
            bad += 1
            continue
        for ri, (want_row, got_row) in enumerate(zip(data, t["rows"])):
            want_row = (want_row + [None] * width)[:width]
            for ci, (w, g) in enumerate(zip(want_row, got_row)):
                typ = t["columns"][ci]["type"]
                got = canon_got(g, typ)
                exp = text_like(w) if typ == "string" else w
                if exp != got and not (exp and got and exp[0] == got[0] == "num" and abs(exp[1] - got[1]) <= 1e-12 * max(1.0, abs(exp[1]))):
                    print(f"MISMATCH {ws.title} r{ri + 2} c{ci + 1} ({typ}): expected {exp!r}, got {got!r}")
                    bad += 1
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1], sys.argv[2]))
