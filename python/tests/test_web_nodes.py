"""Smoke tests that verify the `tavily_client` node is reachable from
Python. Does not hit the live API — only checks registration and basic
configuration parsing.
"""

import json
import os
import subprocess
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
GRAPH_PATH = REPO_ROOT / "tests" / "graphs" / "web" / "tavily_search_basic.json"


def test_graph_file_exists():
    assert GRAPH_PATH.exists(), f"missing {GRAPH_PATH}"


def test_graph_has_tavily_client_toolkit_config():
    graph = json.loads(GRAPH_PATH.read_text())
    agent_nodes = [
        node for node in graph["nodes"].values() if node["type"] == "llm_call"
    ]
    assert agent_nodes, "graph has no llm_call node"
    cfg = agent_nodes[0]["config"]["tool_configurations"]["web"]
    assert cfg["node_type"] == "tavily_client"
    assert cfg["expose_sub_tools"] in ("all", ["search", "fetch"])


@pytest.mark.skipif(
    not os.environ.get("TAVILY_API_KEY") or not os.environ.get("ANTHROPIC_API_KEY"),
    reason="Live API keys not set",
)
def test_cli_runs_graph_end_to_end():
    """Shell out to the `dag_engine` CLI. Covers the full Python →
    CLI → Rust path minus the Python binding itself (which is tested
    separately in python/tests/test_python_bindings.py)."""
    result = subprocess.run(
        [
            "cargo",
            "run",
            "--quiet",
            "--manifest-path",
            "src/libs/colmena/Cargo.toml",
            "--bin",
            "dag_engine",
            "--",
            "run",
            str(GRAPH_PATH),
        ],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        timeout=180,
    )
    assert result.returncode == 0, f"CLI failed:\n{result.stderr}"
