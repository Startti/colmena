"""Thin pure-Python wrapper around colmena.documents that adds pandas helpers.

Usage:

    import colmena_documents as cd
    import pandas as pd

    df = pd.DataFrame({"Product": ["Apple", "Pear"], "Qty": [10, 20]})
    sheet_id = cd.add_sheet("art_<id>", "PythonSheet")
    cd.write_sheet("art_<id>", sheet_id, df)
    out = cd.read_sheet("art_<id>", sheet_id)
"""

from colmena import documents as _native


def list_sheets(artifact_id):
    """List sheets in an artifact. Returns a list of {"sheet_id", "name"} dicts."""
    return _native.list_sheets(artifact_id)


def read_sheet(artifact_id, sheet_id):
    """Read a sheet's cells into a pandas DataFrame.

    First row of the sheet is treated as the column header. Subsequent rows
    become DataFrame rows. Empty cells become NaN.
    """
    import pandas as pd

    flat = _native.read_sheet(artifact_id, sheet_id)
    if not flat:
        return pd.DataFrame()

    # Group cells by row index (1-based in A1 notation).
    by_row = {}
    for addr, value in flat.items():
        # Split addr into letters (column) and digits (row).
        col_letters = "".join(c for c in addr if c.isalpha())
        row_part = "".join(c for c in addr if c.isdigit())
        if not col_letters or not row_part:
            continue
        try:
            row = int(row_part)
        except ValueError:
            continue
        by_row.setdefault(row, {})[col_letters] = value

    if not by_row:
        return pd.DataFrame()

    sorted_rows = sorted(by_row.items())
    # Row 1 is the header.
    _, header_row = sorted_rows[0]
    column_keys = sorted(header_row.keys())
    columns = [header_row[k] for k in column_keys]

    data_rows = []
    for _row_idx, cells in sorted_rows[1:]:
        data_rows.append([cells.get(k) for k in column_keys])

    return pd.DataFrame(data_rows, columns=columns)


def write_sheet(artifact_id, sheet_id, df, mode="replace"):
    """Write a pandas DataFrame to a sheet.

    Columns become row 1 (header). Subsequent rows are the DataFrame's
    values. `mode = "replace"` overwrites existing cells; "append" preserves
    them at addresses that aren't being written to.
    """
    columns = [str(c) for c in df.columns]
    # Convert each cell to a python-native type the native binding can extract.
    rows = []
    for record in df.values.tolist():
        rows.append([
            None if (isinstance(v, float) and v != v)  # NaN check
            else v
            for v in record
        ])
    _native.write_sheet(artifact_id, sheet_id, columns, rows, mode)


def add_sheet(artifact_id, name):
    """Append a new sheet. Returns the generated sheet_id."""
    return _native.add_sheet(artifact_id, name)
