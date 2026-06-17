"""Tests for `colmena.stream_dag` — the async iterator over a DAG's SSE-mapped events.

Each yielded item is a dict with a `type` (`node-start`, `node-end`, `text-delta`,
`finish`, …), identical to what the HTTP `serve_dag` endpoint streams. Pure-math
graphs run with no API keys / network.
"""

import json
import os

import colmena
import pytest

# mock_input 5 -> exponential^3 -> log  => 125
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

# trigger_webhook -> exponential^3 -> log  (so inject_payload has a node to land on)
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


def _write_graph(tmp_path, graph) -> str:
    path = tmp_path / "graph.json"
    path.write_text(json.dumps(graph))
    return str(path)


async def _collect(graph_arg, **kwargs):
    stream = await colmena.stream_dag(graph_arg, **kwargs)
    events = []
    async for ev in stream:
        assert isinstance(ev, dict), f"event is not a dict: {ev!r}"
        assert "type" in ev, f"event has no 'type': {ev!r}"
        events.append(ev)
    return events


async def test_stream_dag_emits_node_lifecycle_and_finish(tmp_path):
    """A pure-math graph streams node-start/node-end per node plus a terminal finish."""
    events = await _collect(_write_graph(tmp_path, POWER_GRAPH))
    types = [e["type"] for e in events]

    assert "node-start" in types and "node-end" in types

    finish = [e for e in events if e["type"] == "finish"]
    assert len(finish) == 1, f"expected exactly one finish; got types={types}"
    assert finish[0]["output"]["pow_step"]["output"] == 125.0


async def test_stream_dag_accepts_inmemory_dict():
    """stream_dag accepts an in-memory graph dict, not just a file path."""
    events = await _collect(POWER_GRAPH)
    assert any(e["type"] == "finish" for e in events)


async def test_stream_dag_inject_payload(tmp_path):
    """inject_payload reaches the trigger node and drives the computation (7**3)."""
    events = await _collect(
        _write_graph(tmp_path, WEBHOOK_GRAPH), inject_payload={"input": 7}
    )
    finish = [e for e in events if e["type"] == "finish"]
    assert len(finish) == 1
    assert finish[0]["output"]["pow_step"]["output"] == 343.0


async def test_stream_dag_missing_file_raises():
    """A non-existent graph file raises DagException when awaited (no panic)."""
    with pytest.raises(colmena.DagException):
        await colmena.stream_dag("does/not/exist.json")


@pytest.mark.skipif(
    not os.getenv("GEMINI_API_KEY"),
    reason="needs GEMINI_API_KEY (real agent stream)",
)
async def test_stream_dag_agent_emits_text_delta():
    """A streaming LLM agent emits text-delta parts through stream_dag."""
    events = await _collect("tests/graphs/agents/agent_with_tools_gemini.json")
    types = {e["type"] for e in events}
    assert "text-delta" in types, f"no text-delta; got {sorted(types)}"
    assert any(e["type"] == "finish" for e in events)
