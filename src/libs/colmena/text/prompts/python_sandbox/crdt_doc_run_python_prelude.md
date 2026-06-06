# === colmena auto-prelude (do not modify) ===
import pandas as pd
import numpy as np
from scipy import stats

dfs = {k: pd.DataFrame(v) for k, v in _dfs_raw.items()}
del _dfs_raw

# === user code starts here ===
