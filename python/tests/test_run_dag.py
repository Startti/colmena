"""Tests for the DAG-engine Python bindings: `run_dag` and `validate_graph`.

Regression coverage for the bug where `colmena.run_dag` panicked on every graph
because `engine.run_dag` routed to the deprecated `DagRunUseCase::execute()` stub
instead of draining `execute_stream` (fixed in engine.rs).

These graphs are pure-math (mock_input / exponential / log) so they run with no
API keys and no network.
"""

import json

import colmena
import pytest

# A pure-math graph: mock_input 5 -> exponential^3 -> log  => 125
POWER_GRAPH = {
    "nodes": {
        "start": {"type": "mock_input", "config": {"input": 5}},
        "pow_step": {"type": "exponential", "config": {"exponent": 3}},
        "log_result": {"type": "log"},
    },
    "edges": [
        {"from": "start", "to": "pow_step"},
        {"from": "pow_step", "to": "log_result"},
    ],
}


def _write_graph(tmp_path, graph) -> str:
    path = tmp_path / "graph.json"
    path.write_text(json.dumps(graph))
    return str(path)


def test_run_dag_returns_final_output(tmp_path):
    """run_dag executes the graph and returns the final output as JSON string."""
    result = json.loads(colmena.run_dag(_write_graph(tmp_path, POWER_GRAPH)))

    # 5 ** 3 == 125, surfaced both at the exponential node and the log node.
    assert result["pow_step"]["output"] == 125.0
    assert result["log_result"] == 125.0
    # Every run carries an opaque session id.
    assert "__colmena_session_id" in result


def test_run_dag_missing_file_raises():
    """A non-existent graph file raises DagException, not a panic."""
    with pytest.raises(colmena.DagException):
        colmena.run_dag("does/not/exist.json")


def test_validate_graph_accepts_valid_dict():
    """validate_graph returns None for a structurally valid graph."""
    assert colmena.validate_graph(POWER_GRAPH) is None


def test_validate_graph_rejects_invalid_dict():
    """validate_graph raises DagException for a malformed graph."""
    with pytest.raises(colmena.DagException):
        colmena.validate_graph({"nodes": "not-a-dict"})


def test_default_registry_lists_node_types():
    """default_registry exposes the registered node types (no DB needed)."""
    node_types = colmena.default_registry().node_types()
    assert isinstance(node_types, list)
    # Nodes used by the test graph must be registered.
    for nt in ("mock_input", "exponential", "log"):
        assert nt in node_types
