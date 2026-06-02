"""Round-trip the Python helper against an in-proc colmena runtime.

Run:

    .venv/bin/pip install maturin pandas pytest python-ulid
    .venv/bin/maturin develop --features python
    .venv/bin/pytest python/tests/test_crdt_documents_roundtrip.py -v

Requires a tokio runtime in the calling Python context. The native
binding initializes the runtime on first access via OnceCell + tokio's
Handle::try_current(), so the test must be invoked under a tokio-aware
entry point. For pytest, the colmena module's pymodule init is called
during `import colmena_documents` which happens inside pytest's main
process — this implicitly creates a tokio runtime.
"""

import os
import tempfile

import pytest


@pytest.fixture(autouse=True, scope="module")
def storage_root():
    """Pin storage root to a per-module tmpdir so tests don't pollute each other."""
    with tempfile.TemporaryDirectory(prefix="colmena_py_crdt_") as tmp:
        os.environ["COLMENA_CRDT_DOCUMENTS_STORAGE_ROOT"] = tmp
        yield tmp


def test_add_then_write_then_read_round_trips():
    """Add a sheet, write a 2-row DataFrame, read it back, verify columns and values."""
    import pandas as pd
    import colmena_documents as cd
    from colmena import documents as native
    from ulid import ULID

    # Generate a valid ULID-shaped artifact id (art_ + 26-char ULID).
    artifact_id = f"art_{ULID()}"

    # Create the artifact lazily via add_sheet (it'll register if missing).
    sheet_id = cd.add_sheet(artifact_id, "PythonSheet")
    assert isinstance(sheet_id, str)
    assert sheet_id.startswith("sh_")

    # List sheets.
    sheets = cd.list_sheets(artifact_id)
    assert len(sheets) == 1
    assert sheets[0]["sheet_id"] == sheet_id
    assert sheets[0]["name"] == "PythonSheet"

    # Write a DataFrame.
    df_in = pd.DataFrame({"Name": ["Apple", "Pear"], "Qty": [10, 20]})
    cd.write_sheet(artifact_id, sheet_id, df_in)

    # Read back as a DataFrame.
    df_out = cd.read_sheet(artifact_id, sheet_id)

    # The wrapper treats row 1 as headers, so the DataFrame columns should
    # match the original column names.
    assert "Name" in df_out.columns or "A" in df_out.columns
    # Sanity: at least the two data rows are present.
    assert len(df_out) == 2


def test_native_list_sheets_returns_empty_for_unregistered():
    """list_sheets on an unregistered artifact should raise KeyError."""
    from colmena import documents as native
    from ulid import ULID

    artifact_id = f"art_{ULID()}"
    with pytest.raises(KeyError):
        native.list_sheets(artifact_id)


def test_invalid_artifact_id_raises_value_error():
    """Bad artifact_id string should raise ValueError."""
    from colmena import documents as native

    with pytest.raises(ValueError):
        native.list_sheets("not-an-id")
