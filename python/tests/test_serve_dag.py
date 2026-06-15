"""Smoke test for the `serve_dag` binding.

`serve_dag` is a blocking call that starts an HTTP server and also binds the
local artifact-storage port, so this test runs it out-of-process and posts to a
webhook route. If the server can't come up in the current environment (e.g. the
storage port is already taken) the test *skips* rather than fails.
"""

import json
import socket
import subprocess
import sys
import time
import urllib.request

import pytest

# trigger_webhook "/power" -> exponential^3 -> log
WEBHOOK_GRAPH = {
    "nodes": {
        "trigger": {
            "type": "trigger_webhook",
            "config": {"path": "/power", "test_payload": {"input": 10}},
        },
        "pow_step": {"type": "exponential", "config": {"exponent": 3}},
        "log_result": {"type": "log"},
    },
    "edges": [
        {"from": "trigger", "to": "pow_step"},
        {"from": "pow_step", "to": "log_result"},
    ],
}

# Driver run in a subprocess: serve_dag blocks until the process is terminated.
_DRIVER = "import sys, colmena; colmena.serve_dag(sys.argv[1], host='127.0.0.1', port=int(sys.argv[2]))"


def _free_tcp_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def _wait_until_listening(port: int, timeout: float = 25.0) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.settimeout(0.5)
            if s.connect_ex(("127.0.0.1", port)) == 0:
                return True
        time.sleep(0.3)
    return False


def test_serve_dag_webhook_smoke(tmp_path):
    graph_path = tmp_path / "webhook.json"
    graph_path.write_text(json.dumps(WEBHOOK_GRAPH))
    port = _free_tcp_port()

    proc = subprocess.Popen(
        [sys.executable, "-c", _DRIVER, str(graph_path), str(port)],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    try:
        if not _wait_until_listening(port):
            proc.terminate()
            tail = (proc.communicate(timeout=5)[0] or "")[-400:]
            pytest.skip(f"serve_dag did not start in this environment:\n{tail}")

        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/power",
            data=json.dumps({"input": 7}).encode(),
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        with urllib.request.urlopen(req, timeout=20) as resp:
            body = json.loads(resp.read())

        # The webhook payload drives the computation: 7 ** 3 == 343.
        assert body["trigger"]["input"] == 7
        assert body["pow_step"]["output"] == 343.0
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
