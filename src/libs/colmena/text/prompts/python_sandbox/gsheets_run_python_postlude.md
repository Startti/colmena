
# === user code ends ===

# === colmena auto-postlude ===
__col_user_output = output if 'output' in dir() else None

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
            # Non-DataFrame entries are silently skipped; the dispatcher
            # surfaces a warning if everything ended up skipped.

output = {
    'user_output': __col_user_output,
    'output_sheets': __col_output_sheets,
}
