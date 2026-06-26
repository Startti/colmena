
# === user code ends ===

# === colmena auto-postlude ===
__col_user_output = output if 'output' in dir() else None

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

output = {
    'user_output': __col_user_output,
    'output_sheets': __col_output_sheets,
}
