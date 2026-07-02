# === colmena auto-prelude (do not modify) ===
import pandas as pd
import numpy as np
from scipy import stats

# Each binding is already bound as `<var>` (a list of {col: val} dicts).
# `_loaded_columns` is a dict of {var: [col, ...]} for reference.
# A binding may come from an attachment, Google Sheets, a SQL SELECT, or
# inline `data` you passed — either way it is a list of {col: val} dicts.
# Build a DataFrame with pd.DataFrame(<var>).
#
# Assign any of these globals to return results:
#   output             — any JSON-serializable value (what the LLM sees)
#   output_tables       — dict of {"schema.table": DataFrame or spec} to
#                          write into SQL tables
#   output_sheets        — dict of {"sheet name": DataFrame or spec} to
#                          write into Google Sheets
#   output_attachments   — dict of {"name.ext": DataFrame or spec} to
#                          register as new conversation attachments

# === user code starts here ===
