
# === user code ends ===

# === colmena auto-postlude ===
__col_user_output = output if 'output' in dir() else None

# Coerce a user `output` into a JSON-safe Python value BEFORE the Rust side
# runs depythonize (which cannot convert pandas/numpy types). Mirrors the
# auto-conversions promised in the tool description: DataFrame →
# to_dict('records'), Series → list, numpy scalar → .item(), numpy array →
# list. Recurses into dicts/lists/tuples so nested numpy values (e.g.
# `output = {"count": df['n'].sum()}`) are coerced too. Uses explicit
# isinstance on the real pandas/numpy types (not `hasattr('to_dict')`) so a
# Series maps to a flat list and unrelated objects with a `to_dict` method
# are not hijacked. Anything else passes through untouched.
def __col_json_safe(v):
    if v is None:
        return None
    try:
        import pandas as _pd
        if isinstance(v, _pd.DataFrame):
            return __col_json_safe(v.to_dict('records'))
        if isinstance(v, _pd.Series):
            return __col_json_safe(v.tolist())
    except Exception:
        pass
    try:
        import numpy as _np
        if isinstance(v, _np.generic):
            return v.item()
        if isinstance(v, _np.ndarray):
            return __col_json_safe(v.tolist())
    except Exception:
        pass
    if isinstance(v, dict):
        return {k: __col_json_safe(_x) for k, _x in v.items()}
    if isinstance(v, (list, tuple)):
        return [__col_json_safe(_x) for _x in v]
    return v

__col_output_sheets = None
if 'output_sheets' in dir() and output_sheets is not None:
    import pandas as _pd
    if isinstance(output_sheets, dict):
        __col_output_sheets = {}
        for k, v in output_sheets.items():
            entry = None
            # Shape 1: bare DataFrame → defaults to mode="replace".
            if isinstance(v, _pd.DataFrame):
                entry = {
                    'mode': 'replace',
                    'df_records': v.to_dict('records'),
                    'df_cols': list(v.columns),
                    'df_index': [int(i) if isinstance(i, (int, float)) else i for i in v.index],
                    'key': None,
                    'columns': None,
                    'strict_match': False,
                    'allow_schema_change': False,
                }
            # Shape 2: spec dict with required "mode" and "df".
            elif isinstance(v, dict):
                mode = v.get('mode', 'replace')
                df = v.get('df')
                if isinstance(df, _pd.DataFrame):
                    entry = {
                        'mode': str(mode),
                        'df_records': df.to_dict('records'),
                        'df_cols': list(df.columns),
                        'df_index': [int(i) if isinstance(i, (int, float)) else i for i in df.index],
                        'key': v.get('key'),
                        'columns': v.get('columns'),
                        'strict_match': bool(v.get('strict_match', False)),
                        'allow_schema_change': bool(v.get('allow_schema_change', False)),
                    }
                else:
                    # df missing or wrong type — surface a marker the
                    # dispatcher turns into a clear error.
                    entry = {
                        'mode': str(mode),
                        '_postlude_error': 'spec dict missing a pandas DataFrame in the "df" field',
                    }
            # Anything else (list, scalar, etc.) is silently skipped;
            # the dispatcher logs a warning if all entries skipped.
            if entry is not None:
                __col_output_sheets[str(k)] = entry

__col_output_tables = None
if 'output_tables' in dir() and output_tables is not None:
    import pandas as _pd
    if isinstance(output_tables, dict):
        __col_output_tables = {}
        for k, v in output_tables.items():
            entry = None
            # Shape 1: bare DataFrame → defaults to mode="append".
            if isinstance(v, _pd.DataFrame):
                entry = {
                    'mode': 'append',
                    'df': v.to_dict('records'),
                    'key': None,
                    'columns': None,
                }
            # Shape 2: spec dict with "mode"/"df"/optional "key"/"columns".
            elif isinstance(v, dict):
                mode = v.get('mode', 'append')
                df = v.get('df')
                if isinstance(df, _pd.DataFrame):
                    entry = {
                        'mode': str(mode),
                        'df': df.to_dict('records'),
                        'key': v.get('key'),
                        'columns': v.get('columns'),
                    }
                else:
                    entry = {
                        'mode': str(mode),
                        '_postlude_error': 'spec dict missing a pandas DataFrame in the "df" field',
                    }
            # Anything else (list, scalar, etc.) is silently skipped;
            # the dispatcher logs a warning if all entries skipped.
            if entry is not None:
                __col_output_tables[str(k)] = entry

__col_output_attachments = None
if 'output_attachments' in dir() and output_attachments is not None:
    import pandas as _pd
    if isinstance(output_attachments, dict):
        __col_output_attachments = {}
        for k, v in output_attachments.items():
            entry = None
            # Shape 1: bare DataFrame.
            if isinstance(v, _pd.DataFrame):
                entry = {
                    'df': v.to_dict('records'),
                }
            # Shape 2: spec dict with "df" and optional "delimiter".
            elif isinstance(v, dict):
                df = v.get('df')
                if isinstance(df, _pd.DataFrame):
                    entry = {
                        'df': df.to_dict('records'),
                        'delimiter': v.get('delimiter'),
                    }
                else:
                    entry = {
                        '_postlude_error': 'spec dict missing a pandas DataFrame in the "df" field',
                    }
            # Anything else (list, scalar, etc.) is silently skipped;
            # the dispatcher logs a warning if all entries skipped.
            if entry is not None:
                __col_output_attachments[str(k)] = entry

output = {
    'user_output': __col_json_safe(__col_user_output),
    'output_sheets': __col_output_sheets,
    'output_tables': __col_output_tables,
    'output_attachments': __col_output_attachments,
}
