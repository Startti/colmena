
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
            if isinstance(v, _pd.DataFrame):
                __col_output_sheets[str(k)] = {
                    'records': v.to_dict('records'),
                    'cols': list(v.columns),
                }

output = {
    'user_output': __col_user_output,
    'sheet_records': __col_sheet_records,
    'sheet_cols': __col_sheet_cols,
    'output_sheets': __col_output_sheets,
}
