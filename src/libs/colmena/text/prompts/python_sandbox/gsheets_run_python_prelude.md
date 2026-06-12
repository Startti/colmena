# === colmena auto-prelude (do not modify) ===
import pandas as pd
import numpy as np
from scipy import stats

# Each binding is already bound as `<var>` (a list of {col: val} dicts).
# `_gsheets_loaded_columns` is a dict of {var: [col, ...]} for reference.
# A binding may come from a sheet OR from inline `data` you passed — either way
# it is a list of {col: val} dicts. Build a DataFrame with pd.DataFrame(<var>).

# === user code starts here ===
