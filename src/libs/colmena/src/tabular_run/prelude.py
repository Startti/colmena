# === colmena large-table prelude (trusted; dark behind COLMENA_LARGE_TABULAR) ===
# Gives the model's code `tables`, lazy handles over the prepared Parquet parts
# staged at `_ct_data_dir`. Nothing is loaded by default: a handle holds no data,
# `read` needs columns and has a size limit, `parts` is the only unbounded path.
# Uses only what restricted mode allows; the user's code never imports pyarrow.

import re as _re


class LargeTableError(ValueError):
    pass


_CT_MIB = 1024 * 1024
# Bytes a decoded column takes in pandas relative to its Arrow size, by type
# (docs/developer_guide/54_tabular_prepare.md, Manifest). Strings add the Python
# object header per row.
_CT_FACTOR = {'int': 1, 'float': 1, 'bool': 8, 'date': 2, 'timestamp': 1, 'string': 1}
_CT_STRING_OVERHEAD = 57
# Arrow's table and the pandas frame built from it exist together for a moment.
_CT_PEAK = 2


def _ct_fail(message):
    raise LargeTableError(message)


def _ct_guide(name):
    return (
        "`%s` is a handle to a large table, not a DataFrame. Use "
        "`tables['%s'].read(columns=[...], filters=[...])` for the columns you need, or "
        "`for part in tables['%s'].parts(columns=[...])` and combine per-part results."
        % (name, name, name)
    )


def _ct_types(t):
    return {c['name']: c['type'] for c in t['columns']}


def _ct_estimate(t, columns):
    rows = t['rows']
    total = 0
    by_name = {c['name']: c for c in t['columns']}
    for name in columns:
        c = by_name[name]
        size = c['in_memory_bytes'] * _CT_FACTOR[c['type']]
        if c['type'] == 'string':
            size += rows * _CT_STRING_OVERHEAD
        total += size
    return total * _CT_PEAK


def _ct_columns(t, columns, required):
    if columns is None:
        if required:
            _ct_fail(
                "read() needs `columns=[...]`: this table has %d columns and is not loaded "
                "whole. Columns: %s" % (len(t['columns']), _ct_listing([c['name'] for c in t['columns']]))
            )
        return None
    if isinstance(columns, str) or not isinstance(columns, (list, tuple)) or len(columns) == 0:
        _ct_fail("`columns` must be a non-empty list of column names")
    known = {c['name'] for c in t['columns']}
    for name in columns:
        if not isinstance(name, str) or name not in known:
            _ct_fail(
                "no column named %r in `%s`. Columns: %s"
                % (name, t['name'], _ct_listing([c['name'] for c in t['columns']]))
            )
    return list(columns)


def _ct_listing(names):
    shown = ', '.join(repr(n) for n in names[:30])
    return shown + (', ... (%d more)' % (len(names) - 30) if len(names) > 30 else '')


def _ct_filters(filters):
    if filters is not None and not isinstance(filters, (list, tuple)):
        _ct_fail("`filters` must be a list of (column, operator, value) tuples")
    return filters


def _ct_read(t, part, columns, filters):
    import pandas as pd
    path = '%s/t%d/part-%05d.parquet' % (_ct_data_dir, t['index'], part)
    return pd.read_parquet(
        path, columns=columns, filters=filters,
        use_threads=False, pre_buffer=False, memory_map=False,
    )


class _Table:
    def __init__(self, t):
        object.__setattr__(self, '_t', t)

    @property
    def name(self):
        return self._t['name']

    @property
    def rows(self):
        return self._t['rows']

    @property
    def n_parts(self):
        return self._t['parts']

    @property
    def columns(self):
        return [c['name'] for c in self._t['columns']]

    @property
    def dtypes(self):
        return _ct_types(self._t)

    def head(self, n=5, columns=None):
        if not isinstance(n, int) or n < 1 or n > 1000:
            _ct_fail("`n` must be an integer from 1 to 1000")
        cols = _ct_columns(self._t, columns, False)
        return _ct_read(self._t, 0, cols, None).head(n)

    def read(self, columns=None, filters=None):
        t = self._t
        cols = _ct_columns(t, columns, True)
        filters = _ct_filters(filters)
        need = _ct_estimate(t, cols)
        if need > _ct_read_max:
            _ct_fail(
                "`%s` is too large to load whole (%d rows, about %d MiB for these columns; limit "
                "%d MiB). Select fewer columns with `tables['%s'].read(columns=[...], filters=[...])`, "
                "or loop `for part in tables['%s'].parts(columns=[...])` and combine per-part results."
                % (t['name'], t['rows'], -(-need // _CT_MIB), _ct_read_max // _CT_MIB, t['name'], t['name'])
            )
        import pandas as pd
        frames = [_ct_read(t, p, cols, filters) for p in range(t['parts'])]
        return frames[0] if len(frames) == 1 else pd.concat(frames, ignore_index=True)

    def parts(self, columns=None, filters=None):
        t = self._t
        cols = _ct_columns(t, columns, False)
        filters = _ct_filters(filters)
        for p in range(t['parts']):
            yield _ct_read(t, p, cols, filters)

    def __getattr__(self, attr):
        _ct_fail(_ct_guide(self._t['name']))

    def __getitem__(self, key):
        _ct_fail(_ct_guide(self._t['name']))

    def __iter__(self):
        _ct_fail(_ct_guide(self._t['name']))

    def __len__(self):
        _ct_fail(_ct_guide(self._t['name']))

    def __repr__(self):
        t = self._t
        return '<table %s: %d rows, %d columns, %d parts>' % (t['name'], t['rows'], len(t['columns']), t['parts'])


class _Tables:
    def __init__(self, tables):
        self._tables = list(tables)

    @property
    def names(self):
        return [t['name'] for t in self._tables]

    def _find(self, name):
        for t in self._tables:
            if t['name'] == name:
                return t
        lower = [t for t in self._tables if t['name'].lower() == str(name).lower()]
        if len(lower) == 1:
            return lower[0]
        _ct_fail("no table named %r. Tables: %s" % (name, _ct_listing(self.names)))

    def schema(self, name):
        t = self._find(name)
        return {
            'name': t['name'], 'rows': t['rows'], 'parts': t['parts'],
            'columns': [{'name': c['name'], 'type': c['type']} for c in t['columns']],
        }

    def __getitem__(self, name):
        return _Table(self._find(name))

    def __contains__(self, name):
        return name in self.names

    def __len__(self):
        return len(self._tables)

    def __iter__(self):
        _ct_fail("`tables` is not iterable. Use `tables.names` for the table names.")

    def __repr__(self):
        return '<tables: %s>' % ', '.join(self.names)


class _NoDf:
    def _no(self):
        _ct_fail(
            "`df` is not loaded: this file is large. Use `tables.names`, `tables.schema(name)`, "
            "`tables[name].read(columns=[...])` or `for part in tables[name].parts(columns=[...])`."
        )

    def __getattr__(self, attr):
        self._no()

    def __getitem__(self, key):
        self._no()

    def __iter__(self):
        self._no()

    def __len__(self):
        self._no()

    def __repr__(self):
        self._no()


_CT_OUT_NAME = _re.compile(r'^[A-Za-z0-9_-]{1,48}$')
_ct_emitted = []


def _ct_file_size(path):
    # The size of a file without reading it: a read-only map, which needs no open().
    import numpy as _np
    try:
        return int(_np.memmap(path, dtype='uint8', mode='r').shape[0])
    except (ValueError, OSError):
        return 0


def emit_table(data, name, format='csv'):
    """Write a DataFrame (or, for csv, an iterable of DataFrames) to a file that
    is returned with the answer. Limits: a few files, each and all together under
    a size; a file over a limit is refused here, and dropped by the reader anyway."""
    if format not in ('csv', 'parquet'):
        _ct_fail("emit_table: format must be 'csv' or 'parquet'")
    if not isinstance(name, str) or not _CT_OUT_NAME.match(name):
        _ct_fail("emit_table: name must be 1 to 48 letters, digits, '_' or '-'")
    if len(_ct_emitted) >= _ct_out_files:
        _ct_fail("emit_table: at most %d files can be returned" % _ct_out_files)
    file_name = '%s.%s' % (name, format)
    if any(e['name'] == file_name for e in _ct_emitted):
        _ct_fail("emit_table: %s was already written" % file_name)
    import pandas as pd
    single = isinstance(data, pd.DataFrame)
    if format == 'parquet' and not single:
        _ct_fail("emit_table: parquet takes one DataFrame; use format='csv' for several parts, or combine them first")
    path = '%s/%s' % (_ct_out_dir, file_name)
    already = sum(e['size'] for e in _ct_emitted)
    rows, first, dtypes, size = 0, True, None, 0
    for frame in ([data] if single else data):
        if not isinstance(frame, pd.DataFrame):
            _ct_fail("emit_table takes a DataFrame or an iterable of DataFrames")
        if dtypes is None:
            dtypes = {str(c): str(t) for c, t in list(frame.dtypes.items())[:200]}
        if format == 'csv':
            frame.to_csv(path, mode='w' if first else 'a', header=first, index=False)
        else:
            frame.to_parquet(path, index=False)
        first = False
        rows += len(frame)
        size = _ct_file_size(path)
        if size > _ct_out_file_max or already + size > _ct_out_total_max:
            _ct_fail(
                "emit_table: %s is over the limit (%d MiB for a file, %d MiB for all files); "
                "return fewer rows or aggregate first"
                % (file_name, _ct_out_file_max // _CT_MIB, _ct_out_total_max // _CT_MIB)
            )
    if first:
        _ct_fail("emit_table: there was nothing to write")
    _ct_emitted.append({'name': file_name, 'format': format, 'rows': rows, 'dtypes': dtypes, 'size': size})


tables = _Tables(_ct_tables)
df = _NoDf()
# === end of the large-table prelude ===
