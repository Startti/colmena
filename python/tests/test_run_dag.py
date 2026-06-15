"""Tests for the DAG-engine Python bindings: `run_dag` and `validate_graph`.

Regression coverage for the bug where `colmena.run_dag` panicked on every graph
because `engine.run_dag` routed to the deprecated `DagRunUseCase::execute()` stub
instead of draining `execute_stream` (fixed in engine.rs).

These graphs are pure-math (mock_input / exponential / log) so they run with no
API keys and no network.
"""

import json
import os
import uuid

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

# Same math, but fed by a trigger_webhook so `inject_payload` has a node to land on.
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

# input -> suspend(approve_continue) -> log. Run 1 suspends; run 2 resumes.
SUSPEND_GRAPH = {
    "nodes": {
        "start": {"type": "input", "config": {"msg": "step 1"}},
        "controller": {
            "type": "suspend",
            "config": {"id": "approve_continue", "question": "Approve?"},
        },
        "final": {"type": "log"},
    },
    "edges": [
        {"from": "start", "to": "controller"},
        {"from": "controller", "to": "final"},
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


def test_run_dag_accepts_inmemory_dict():
    """run_dag runs a graph dict directly, with no file written to disk."""
    result = json.loads(colmena.run_dag(POWER_GRAPH))
    assert result["pow_step"]["output"] == 125.0
    assert result["log_result"] == 125.0


def test_run_dag_missing_file_raises():
    """A non-existent graph file raises DagException, not a panic."""
    with pytest.raises(colmena.DagException):
        colmena.run_dag("does/not/exist.json")


def test_run_dag_rejects_garbage_arg():
    """A non-str / non-dict graph argument raises DagException."""
    with pytest.raises(colmena.DagException):
        colmena.run_dag(12345)


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


def test_run_dag_inject_payload(tmp_path):
    """inject_payload lands on the trigger_webhook node as the incoming payload."""
    result = json.loads(
        colmena.run_dag(
            _write_graph(tmp_path, WEBHOOK_GRAPH),
            inject_payload={"input": 7},
        )
    )
    # The injected payload drives the computation: 7 ** 3 == 343.
    assert result["trigger"]["input"] == 7
    assert result["pow_step"]["output"] == 343.0


@pytest.mark.skipif(
    not os.getenv("DATABASE_URL"),
    reason="suspend/resume persists DAG state across runs — requires DATABASE_URL",
)
def test_run_dag_suspend_then_resume(tmp_path):
    """A suspend node pauses run 1; run 2 resumes it via a stable agent_session_id."""
    graph_path = _write_graph(tmp_path, SUSPEND_GRAPH)
    agent = f"py_test_suspend_{uuid.uuid4().hex}"

    # Run 1 — suspends at the `approve_continue` node.
    suspended = json.loads(colmena.run_dag(graph_path, agent_session_id=agent))
    assert suspended["__colmena_status"] == "SUSPENDED"
    assert suspended["questions"][0]["id"] == "approve_continue"

    # Run 2 — resume with the canonical Q/A answer format, keyed by agent_session_id.
    answer = "Q[approve_continue]: Approve?\nA[approve_continue]: yes, approved"
    resumed = json.loads(
        colmena.run_dag(graph_path, resume_answer=answer, agent_session_id=agent)
    )
    assert resumed["controller"]["status"] == "resumed"
    assert resumed["final"] == "yes, approved"
