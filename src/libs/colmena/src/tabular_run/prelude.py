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
# Reads force `date_as_object=False` (a date is 8 bytes in pandas, not a Python object) and
# nullable booleans (a value and a mask byte each), so these factors are what the frame is.
_CT_FACTOR = {'int': 1, 'float': 1, 'bool': 16, 'date': 2, 'timestamp': 1, 'string': 1}
_CT_STRING_OVERHEAD = 57
# A column that datetime64[ns] cannot hold stays Python objects: bytes per row.
_CT_OBJECT_BYTES = {'date': 40, 'timestamp': 56}
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
        if c['type'] in ('date', 'timestamp') and _ct_object_column(t, name):
            # Python objects: a date or datetime plus the pointer to it, per row.
            size = rows * _CT_OBJECT_BYTES[c['type']]
        else:
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


def _ct_pyarrow():
    # pyarrow is loaded by pandas; it is reached through pandas' own optional-dependency
    # helper, so no import statement names it. The jail, not this, is the boundary.
    import pandas as pd
    opt = pd.compat._optional.import_optional_dependency
    return pd, opt('pyarrow'), opt('pyarrow.parquet')


def _ct_optional(name):
    import pandas as pd
    return pd.compat._optional.import_optional_dependency(name)


# Microseconds either side of 1970 that datetime64[ns] holds (about 1677-09-22 to 2262-04-11).
_CT_US_MAX = 9223372036854775
_ct_object_cache = {}


def _ct_is_time(pa, ty):
    # A date, or a timestamp not already in nanoseconds (those always fit).
    return pa.types.is_date(ty) or (pa.types.is_timestamp(ty) and ty.unit != 'ns')


def _ct_time_bearing(pa, ty):
    if _ct_is_time(pa, ty):
        return True
    if pa.types.is_list(ty) or pa.types.is_large_list(ty) or pa.types.is_fixed_size_list(ty):
        return _ct_time_bearing(pa, ty.value_type)
    if pa.types.is_struct(ty):
        return any(_ct_time_bearing(pa, ty.field(i).type) for i in range(ty.num_fields))
    if pa.types.is_map(ty):
        return _ct_time_bearing(pa, ty.key_type) or _ct_time_bearing(pa, ty.item_type)
    return False


def _ct_leaf_times(pa, arr):
    # The date and timestamp arrays inside `arr`, nested ones included.
    ty = arr.type
    if _ct_is_time(pa, ty):
        yield arr
    elif pa.types.is_list(ty) or pa.types.is_large_list(ty) or pa.types.is_fixed_size_list(ty):
        for leaf in _ct_leaf_times(pa, arr.flatten()):
            yield leaf
    elif pa.types.is_struct(ty):
        for i in range(ty.num_fields):
            for leaf in _ct_leaf_times(pa, arr.field(i)):
                yield leaf
    elif pa.types.is_map(ty):
        for sub in (arr.keys, arr.items):
            for leaf in _ct_leaf_times(pa, sub):
                yield leaf


def _ct_leaf_fits(pa, pc, arr):
    ty = arr.type
    if pa.types.is_date32(ty):
        ints, per_us = arr.cast(pa.int32()), 86400 * 10 ** 6
    elif pa.types.is_date64(ty):
        ints, per_us = arr.cast(pa.int64()), 1000
    else:
        ints, per_us = arr.cast(pa.int64()), {'s': 10 ** 6, 'ms': 1000, 'us': 1}[ty.unit]
    mm = pc.min_max(ints)
    lo, hi = mm['min'].as_py(), mm['max'].as_py()
    return lo is None or (lo * per_us >= -_CT_US_MAX and hi * per_us <= _CT_US_MAX)


def _ct_us_of(value):
    import datetime
    if isinstance(value, datetime.datetime):
        epoch = datetime.datetime(1970, 1, 1, tzinfo=datetime.timezone.utc if value.tzinfo else None)
        d = value - epoch
        return (d.days * 86400 + d.seconds) * 10 ** 6 + d.microseconds
    if isinstance(value, datetime.date):
        return (value.toordinal() - 719163) * 86400 * 10 ** 6
    return None


def _ct_stats_fit(pf, name):
    # From the footer alone: True / False, or None when it cannot say (no statistics for
    # some row group, a nested column, a value of an unknown kind).
    md = pf.metadata
    index = [i for i in range(md.num_columns) if md.schema.column(i).path == name]
    if len(index) != 1:
        return None
    for r in range(md.num_row_groups):
        st = md.row_group(r).column(index[0]).statistics
        if st is None or not st.has_min_max:
            return None
        for value in (st.min, st.max):
            us = _ct_us_of(value)
            if us is None:
                return None
            if abs(us) > _CT_US_MAX:
                return False
    return True


def _ct_object_column(t, name):
    # Decided ONCE per table and column, for all its parts: does a date or timestamp
    # in it fall outside datetime64[ns]? pandas would wrap such a value into a wrong
    # one, so the column stays Python objects in every part (consistent dtype), and
    # only that column. The footer's statistics decide; a part without them is scanned
    # (that column alone, one chunk at a time).
    key = (t['index'], name)
    if key in _ct_object_cache:
        return _ct_object_cache[key]
    kinds = {c['name']: c['type'] for c in t.get('columns', [])}
    if name in kinds and kinds[name] not in ('date', 'timestamp'):
        return False
    pd, pa, pq = _ct_pyarrow()
    pc = _ct_optional('pyarrow.compute')
    field = pq.read_schema(_ct_path(t, 0)).field(name)
    objects = False
    if _ct_time_bearing(pa, field.type):
        flat = _ct_is_time(pa, field.type)
        for part in range(t['parts']):
            fit = _ct_stats_fit(pq.ParquetFile(_ct_path(t, part)), name) if flat else None
            if fit is None:
                column = pq.read_table(_ct_path(t, part), columns=[name]).column(0)
                fit = all(
                    _ct_leaf_fits(pa, pc, leaf)
                    for chunk in column.chunks
                    for leaf in _ct_leaf_times(pa, chunk)
                )
            if not fit:
                objects = True
                break
    _ct_object_cache[key] = objects
    return objects


def _ct_frame(pd, pa, table, t):
    # No Python date objects (about ten times the memory) and no object columns for
    # booleans with nulls. A date or timestamp column datetime64[ns] cannot hold (a
    # 9999-12-31 sentinel, a year before 1677) stays Python objects, that column only
    # and in every part. Nothing is caught here: an out-of-memory is the call's.
    mapper = {pa.bool_(): pd.BooleanDtype()}.get
    objects = [n for n in table.column_names if _ct_object_column(t, n)]
    if not objects:
        return table.to_pandas(date_as_object=False, types_mapper=mapper)
    held = table.select(objects).to_pandas(date_as_object=True, timestamp_as_object=True, types_mapper=mapper)
    rest = [n for n in table.column_names if n not in objects]
    if not rest:
        return held
    frame = table.select(rest).to_pandas(date_as_object=False, types_mapper=mapper)
    return pd.concat([frame, held], axis=1)[table.column_names]


def _ct_path(t, part):
    return '%s/t%d/part-%05d.parquet' % (_ct_data_dir, t['index'], part)


def _ct_read(t, part, columns, filters):
    pd, pa, pq = _ct_pyarrow()
    table = pq.read_table(
        _ct_path(t, part), columns=columns, filters=filters,
        use_threads=False, pre_buffer=False, memory_map=False,
    )
    return _ct_frame(pd, pa, table, t)


def _ct_head(t, n, columns):
    # Only the rows asked for are decoded: batches of `n` rows, taken until `n` rows
    # exist (a row group boundary can make the first batch shorter), never the part.
    pd, pa, pq = _ct_pyarrow()
    batches, have = [], 0
    for batch in pq.ParquetFile(_ct_path(t, 0)).iter_batches(batch_size=n, columns=columns, use_threads=False):
        batches.append(batch)
        have += batch.num_rows
        if have >= n:
            break
    if not batches:
        return _ct_frame(pd, pa, pq.read_table(_ct_path(t, 0), columns=columns), t).head(n)
    return _ct_frame(pd, pa, pa.Table.from_batches(batches), t).head(n)


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
        return _ct_head(self._t, n, cols)

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
        # One part at a time, but every column of it when none are named: estimate
        # that, so a table wide enough to overflow a part is refused with the way out.
        per_part = _ct_estimate(t, cols or [c['name'] for c in t['columns']]) // max(t['parts'], 1)
        if per_part > _ct_read_max:
            _ct_fail(
                "one part of `%s` is too large to load with %s (about %d MiB; limit %d MiB). "
                "Select fewer columns with `tables['%s'].parts(columns=[...])`."
                % (t['name'], 'all its columns' if cols is None else 'these columns',
                   -(-per_part // _CT_MIB), _ct_read_max // _CT_MIB, t['name'])
            )
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


def _ct_schema_column(t, c):
    out = {'name': c['name'], 'type': c['type']}
    if c['type'] in ('date', 'timestamp'):
        # What the column is when read: Python objects when datetime64[ns] cannot hold it.
        out['dtype'] = 'object' if _ct_object_column(t, c['name']) else 'datetime64[ns]'
    return out


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
            'columns': [_ct_schema_column(t, c) for c in t['columns']],
        }

    def __getitem__(self, name):
        return _Table(self._find(name))

    def __contains__(self, name):
        try:
            self._find(name)
            return True
        except LargeTableError:
            return False

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
        size = int(_np.memmap(path, dtype='uint8', mode='r').shape[0])
    except (ValueError, OSError):
        size = 0
    if size == 0:
        # A limit that cannot be checked is not a limit that is met.
        _ct_fail("emit_table: could not check the size of the file just written, so it is not returned")
    return size


def emit_table(data, name, format='csv'):
    """Write a DataFrame (or, for csv, an iterable of DataFrames) to a file that
    is returned with the answer. Limits: a few files, each and all together under
    a size; a file over a limit is refused here, and dropped by the reader anyway."""
    if format not in ('csv', 'parquet'):
        _ct_fail("emit_table: format must be 'csv' or 'parquet'")
    if not isinstance(name, str) or not _CT_OUT_NAME.fullmatch(name):
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
