
# === user code ends ===

# === colmena auto-postlude ===
__col_user_output = output if 'output' in dir() else None
__col_sheet_records = None
__col_sheet_cols = None
if 'output_sheet' in dir() and output_sheet is not None:
    import pandas as _pd
    if isinstance(output_sheet, _pd.DataFrame):
        __col_sheet_records = output_sheet.to_dict('records')
        __col_sheet_cols = list(output_sheet.columns)

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
                        'key': v.get('key'),
                        'columns': v.get('columns'),
                        'strict_match': bool(v.get('strict_match', False)),
                        'allow_schema_change': bool(v.get('allow_schema_change', False)),
                    }
                else:
                    entry = {'mode': str(mode), '_postlude_error': 'spec dict missing pandas DataFrame in "df"'}
            if entry is not None:
                __col_output_sheets[str(k)] = entry

output = {
    'user_output': __col_user_output,
    'sheet_records': __col_sheet_records,
    'sheet_cols': __col_sheet_cols,
    'output_sheets': __col_output_sheets,
}
