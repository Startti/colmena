"""Shared pytest setup for the Python binding tests.

The local dev artifact-storage adapter (`LocalHttpStorageAdapter`, used when
`COLMENA_LOCAL` is set) binds a fixed TCP port — 8765 by default. Tests that
build several engines in one process (e.g. `stream_dag`, which runs on the
shared async runtime and releases its port asynchronously) would otherwise
race on that fixed port. Forcing port 0 makes each engine bind an ephemeral
OS-assigned port, so the tests never collide.

In CI `COLMENA_LOCAL` is unset, so the LocalHttp adapter isn't used and this
is a no-op there.
"""

import os

os.environ["COLMENA_LOCAL_STORAGE_PORT"] = "0"
